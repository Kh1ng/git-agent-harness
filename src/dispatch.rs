use crate::config::{self, GahConfig, Profile};
use crate::{provider, runner, worktree};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DispatchArgs {
    pub profile: String,
    pub mode: String,
    pub backend: String,
    pub target: String,
    pub budget: u32,
    pub dry_run: bool,
    pub config_path: Option<String>,
    /// OpenHands profile name (~/.openhands/profiles/<name>.json); overrides profile default
    pub oh_profile: Option<String>,
}

pub fn run(cfg: &GahConfig, args: &DispatchArgs) -> Result<()> {
    let profile = config::get_profile(cfg, &args.profile)?;

    println!("Profile: {}", profile.display_name);
    println!("Repo:    {}", profile.repo);
    println!("Branch:  {}", profile.default_target_branch);
    println!("Mode:    {}", args.mode);
    println!("Backend: {}", args.backend);
    println!();

    if args.dry_run {
        return dry_run(cfg, profile, args);
    }

    let ts = timestamp();
    let session_dir = PathBuf::from(&profile.artifact_root)
        .join("sessions")
        .join(&ts);
    fs::create_dir_all(&session_dir)?;
    println!("Session: {}", session_dir.display());

    match args.mode.as_str() {
        "improve" | "fix" => improve(cfg, profile, args, &session_dir),
        "pm" => pm(profile, &session_dir),
        "review" => review(profile, args, &session_dir),
        "experiment" => anyhow::bail!("experiment mode not yet implemented"),
        other => anyhow::bail!("unknown mode: {}", other),
    }
}

fn resolve_llm(cfg: &GahConfig, args: &DispatchArgs) -> Result<runner::LlmConfig> {
    if let Some(name) = args.oh_profile.as_deref() {
        let mut llm = runner::load_oh_profile(name)?;
        // env vars always win over the profile file
        if let Ok(v) = std::env::var("LLM_BASE_URL") { llm.base_url = v; }
        if let Ok(v) = std::env::var("LLM_API_KEY")  { llm.api_key  = v; }
        if let Ok(v) = std::env::var("LLM_MODEL")    { llm.model    = v; }
        return Ok(llm);
    }

    let cloud = args.backend == "cloud-coder";
    Ok(runner::LlmConfig {
        base_url: cfg.defaults.llm_base_url(),
        api_key:  cfg.defaults.llm_api_key(),
        model:    cfg.defaults.llm_model(cloud),
    })
}

fn run_backend(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
) -> Result<runner::RunResult> {
    match backend {
        "codex" => runner::run_codex(wt, task, session_dir, &profile.codex_args),
        "claude" => runner::run_claude(wt, task, session_dir, &profile.claude_args),
        _ => runner::run_openhands(wt, task, session_dir, llm, &profile.openhands_args),
    }
}

/// Check all required external binaries exist before starting a long operation.
fn preflight(backend: &str) -> Result<()> {
    let required: &[&str] = &["git"];
    let backend_bin = match backend {
        "codex" => "codex",
        "claude" => "claude",
        _ => "openhands",
    };
    for bin in required.iter().chain(std::iter::once(&backend_bin)) {
        if !runner::backend_available(bin) && Command::new("which").arg(bin).output().map(|o| !o.status.success()).unwrap_or(true) {
            anyhow::bail!("required binary '{}' not found on PATH", bin);
        }
    }
    Ok(())
}

fn improve(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
) -> Result<()> {
    preflight(&args.backend)?;

    let llm = resolve_llm(cfg, args)?;

    let ts = timestamp();
    let branch = format!("gah/{}-{}", profile.repo_id, &ts);
    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);
    let repo = Path::new(&profile.local_path);

    println!(
        "Creating worktree from {}...",
        profile.default_target_branch
    );
    let wt = worktree::create(
        repo,
        &profile.default_target_branch,
        &branch,
        &worktree_base,
    )?;
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);

    let task = build_task(profile, &args.mode, &args.target);
    println!("\nRunning {} backend...", args.backend);

    let result = run_backend(&args.backend, profile, &wt, &task, session_dir, &llm);
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            worktree::cleanup(&wt, repo);
            return Err(e);
        }
    };

    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );

    if result.exit_code != 0 {
        worktree::cleanup(&wt, repo);
        anyhow::bail!("backend exited {}", result.exit_code);
    }

    let has_changes = worktree::has_changes(&wt)?;
    if !has_changes {
        println!("No changes produced — nothing to push.");
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    println!("Changes detected. Committing and pushing...");
    let push_url = profile.push_url();
    worktree::commit_and_push(&wt, &branch, &push_url, &profile.repo_id)?;

    let mr_title = format!("[GAH] {}: {}", args.mode, profile.repo_id);
    let mr_body = format!(
        "## GAH {} mode\n\nBranch: `{}`\nTarget: `{}`\n\nGenerated by `gah dispatch`.",
        args.mode, branch, profile.default_target_branch
    );
    let mr = provider::create_draft_mr(profile, &branch, &mr_title, &mr_body)?;
    println!("Draft MR: {}", mr.url);

    worktree::cleanup(&wt, repo);
    Ok(())
}

fn pm(profile: &Profile, session_dir: &Path) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    let log = git_output(&["log", "--oneline", "-20"], repo).unwrap_or_default();

    let test_count = count_test_files(repo);
    let has_ci = repo.join(".github/workflows").exists()
        || repo.join(".gitlab-ci.yml").exists()
        || repo.join(".ci").exists();

    let readme = repo.join("README.md");
    let readme_text = if readme.exists() {
        let s = fs::read_to_string(&readme).unwrap_or_default();
        s[..s.len().min(2000)].to_string()
    } else {
        String::from("(no README)")
    };

    let report = format!(
        "# PM Report: {}\n\n\
         Repo:           {}\n\
         Branch:         {}\n\
         Test files:     {}\n\
         CI configured:  {}\n\n\
         ## Recent commits\n\
         ```\n{}\n```\n\n\
         ## README\n\
         {}\n",
        profile.display_name,
        profile.repo,
        profile.default_target_branch,
        test_count,
        has_ci,
        log,
        readme_text,
    );

    let out_path = session_dir.join("pm-report.md");
    fs::write(&out_path, &report)?;
    println!("{}", report);
    println!("Written: {}", out_path.display());
    Ok(())
}

fn review(profile: &Profile, args: &DispatchArgs, session_dir: &Path) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    let branch = if args.target.is_empty() {
        git_output(&["rev-parse", "--abbrev-ref", "HEAD"], repo)
            .unwrap_or_else(|_| "HEAD".to_string())
    } else {
        args.target.clone()
    };

    let origin_ref = format!("origin/{}", profile.default_target_branch);
    let _ = git_output(&["fetch", "-q", "origin", "--prune"], repo);

    let diff = git_output(&["diff", &origin_ref, "HEAD"], repo).unwrap_or_default();
    let files =
        git_output(&["diff", "--name-only", &origin_ref, "HEAD"], repo).unwrap_or_default();

    let bundle = session_dir.join("review-bundle");
    fs::create_dir_all(&bundle)?;
    fs::write(bundle.join("diff.patch"), &diff)?;
    fs::write(bundle.join("changed-files.txt"), &files)?;
    fs::write(
        bundle.join("mr-description.md"),
        format!(
            "Branch: {}\nTarget: {}\nRepo: {}",
            branch, profile.default_target_branch, profile.repo
        ),
    )?;

    if diff.is_empty() {
        println!("No diff vs {}. Nothing to review.", origin_ref);
        return Ok(());
    }

    println!(
        "Diff: {} bytes, files: {}",
        diff.len(),
        files.lines().count()
    );

    let prompt = format!(
        "Review this diff for correctness, test coverage, and safety. \
         Repo: {}. Branch: {}. Target: {}.\n\nDiff:\n```\n{}\n```\nChanged files:\n{}",
        profile.repo,
        branch,
        profile.default_target_branch,
        &diff[..diff.len().min(60_000)],
        files,
    );

    // Use the requested backend, fall back to claude if available
    let effective_backend = if args.backend == "auto" || args.backend.is_empty() {
        if which("claude").is_some() { "claude" } else { "openhands" }
    } else {
        &args.backend
    };

    let result = match effective_backend {
        "claude" => Command::new("claude").args(["-p", &prompt]).output().ok(),
        _ => None,
    };

    match result {
        Some(o) if o.status.success() => {
            let review_text = String::from_utf8_lossy(&o.stdout).to_string();
            let report_path = session_dir.join("review-report.md");
            fs::write(&report_path, &review_text)?;
            println!("{}", review_text);
            println!("Written: {}", report_path.display());
        }
        _ => {
            println!("Review bundle written to: {}", bundle.display());
            println!("Run `claude -p \"$(cat {}/diff.patch)\"` to review manually.", bundle.display());
        }
    }

    Ok(())
}

fn dry_run(cfg: &GahConfig, profile: &Profile, args: &DispatchArgs) -> Result<()> {
    println!("DRY RUN — no mutations will be performed\n");
    println!("## What would happen\n");
    let ts = timestamp();
    let branch = format!("gah/{}-{}", profile.repo_id, &ts);
    let session_dir = PathBuf::from(&profile.artifact_root)
        .join("sessions")
        .join(&ts);
    println!("Session dir:  {}", session_dir.display());
    println!("New branch:   {}", branch);
    println!("From:         origin/{}", profile.default_target_branch);
    println!(
        "Worktree:     {}/{}",
        cfg.defaults.worktree_base,
        branch.replace('/', "-")
    );
    match args.mode.as_str() {
        "improve" | "fix" => {
            if let Some(name) = args.oh_profile.as_deref() {
                println!("OH profile:   {} (~/.openhands/profiles/{}.json)", name, name);
            } else {
                let cloud = args.backend == "cloud-coder";
                println!("LLM model:    {}", cfg.defaults.llm_model(cloud));
                println!("LLM base:     {}", cfg.defaults.llm_base_url());
            }
            println!("Backend:      {}", args.backend);
            println!("\nSteps: fetch → worktree → {} → diff → commit → push → draft MR", args.backend);
        }
        "pm" => println!("Steps: git log → test count → CI check → write pm-report.md"),
        "review" => println!("Steps: git diff → bundle → claude review"),
        other => println!("mode '{}': not yet implemented", other),
    }
    println!("\n## Safety\n- No pushes, no MRs, no provider calls (dry run)");
    Ok(())
}

fn build_task(profile: &Profile, mode: &str, target: &str) -> String {
    let mut task = format!(
        "Repository: {} ({})\n\
         Local path: {}\n\
         Target branch: {}\n\
         Mode: {}\n\n\
         Select the highest-priority unstarted task from the backlog, recent CI failures, or test gaps.\n\
         Make a small, focused change. Run tests if a test command exists. Do not push or create MRs.\n",
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
        mode,
    );
    if !target.is_empty() {
        task.push_str(&format!("\nTarget: {}\n", target));
    }
    task
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    worktree::git(args, cwd)
}

fn count_test_files(root: &Path) -> usize {
    count_files_matching(root, &|name: &str| {
        name.starts_with("test_")
            || name.ends_with("_test.py")
            || name.ends_with(".test.ts")
            || name.ends_with(".test.js")
            || name.ends_with(".spec.ts")
            || name.ends_with("_test.rs")
    })
}

fn count_files_matching(dir: &Path, pred: &dyn Fn(&str) -> bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(
                name,
                "target" | ".git" | "node_modules" | "__pycache__" | ".venv"
            ) {
                count += count_files_matching(&path, pred);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if pred(name) {
                count += 1;
            }
        }
    }
    count
}

fn which(cmd: &str) -> Option<String> {
    Command::new("which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}
