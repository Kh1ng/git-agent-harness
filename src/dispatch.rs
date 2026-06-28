use crate::config::{self, GahConfig, Profile};
use crate::models::CandidateArtifact;
use crate::{provider, runner, worktree};
use anyhow::{bail, Context, Result};
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
    pub oh_profile: Option<String>,
    pub model: Option<String>,
    pub retries: u32,
    pub allow_draft_fail: bool,
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
        if let Some(m) = &args.model {
            llm.model = m.clone();
        }
        if let Ok(v) = std::env::var("LLM_BASE_URL") {
            llm.base_url = v;
        }
        if let Ok(v) = std::env::var("LLM_API_KEY") {
            llm.api_key = v;
        }
        if let Ok(v) = std::env::var("LLM_MODEL") {
            llm.model = v;
        }
        return Ok(llm);
    }
    // No profile: apply --model override on top of backend defaults
    if let Some(m) = &args.model {
        let cloud = args.backend == "cloud-coder";
        let llm = runner::LlmConfig {
            base_url: cfg.defaults.llm_base_url(),
            api_key: cfg.defaults.llm_api_key(),
            model: m.clone(),
        };
        return Ok(llm);
    }
    let cloud = args.backend == "cloud-coder";
    Ok(runner::LlmConfig {
        base_url: cfg.defaults.llm_base_url(),
        api_key: cfg.defaults.llm_api_key(),
        model: cfg.defaults.llm_model(cloud),
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

/// Run validation_commands in the worktree. Returns Err(combined output) on first failure.
fn validate(profile: &Profile, wt: &Path) -> Result<()> {
    for cmd_str in &profile.validation_commands {
        let parts = shlex::split(cmd_str).ok_or_else(|| {
            anyhow::anyhow!("invalid command string (unterminated quote?): {}", cmd_str)
        })?;
        let Some((bin, rest)) = parts.split_first() else {
            continue;
        };
        println!("  Validating: {}", cmd_str);
        let out = Command::new(bin)
            .args(rest)
            .current_dir(wt)
            .output()
            .with_context(|| format!("failed to run '{}'", cmd_str))?;
        if !out.status.success() {
            bail!(
                "$ {}
{}{}",
                cmd_str,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        }
    }
    Ok(())
}

fn preflight(backend: &str) -> Result<()> {
    let backend_bin = match backend {
        "codex" => "codex",
        "claude" => "claude",
        _ => "openhands",
    };
    for bin in &["git", backend_bin] {
        let found = Command::new("which")
            .arg(bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !found {
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

    let mut task = build_task(profile, &args.mode, &args.target);
    let max_attempts = args.retries + 1;
    let mut validation_failed = false;
    for attempt in 0..max_attempts {
        println!(
            "\nAttempt {}/{}: running {} backend...",
            attempt + 1,
            max_attempts,
            args.backend
        );
        let attempt_session = session_dir.join(format!("attempt-{}", attempt + 1));
        fs::create_dir_all(&attempt_session)?;

        let result = run_backend(&args.backend, profile, &wt, &task, &attempt_session, &llm);
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
            anyhow::bail!(
                "backend exited {} on attempt {}",
                result.exit_code,
                attempt + 1
            );
        }

        if profile.validation_commands.is_empty() {
            break;
        }

        println!(
            "Running validation ({} commands)...",
            profile.validation_commands.len()
        );
        match validate(profile, &wt) {
            Ok(()) => {
                println!("Validation passed.");
                validation_failed = false;
                break;
            }
            Err(e) => {
                validation_failed = true;
                let failure_output = format!("{:#}", e);
                let failure_path = attempt_session.join("validation-failure.txt");
                fs::write(&failure_path, &failure_output)?;
                println!("Validation failed ({})", failure_path.display());

                if attempt + 1 < max_attempts {
                    println!("Retrying with failure context...");
                    task = format!(
                        "{}\n\n## Retry {}: validation failed\n\nFix the following before completing the task:\n\n```\n{}\n```",
                        task,
                        attempt + 1,
                        &failure_output[..failure_output.len().min(8_000)],
                    );
                } else if args.allow_draft_fail {
                    println!(
                        "Validation still failing; --allow-draft-fail set — pushing as draft."
                    );
                } else {
                    worktree::cleanup(&wt, repo);
                    anyhow::bail!(
                        "validation failed after {} attempt(s). Use --allow-draft-fail to push anyway.\n\n{}",
                        max_attempts,
                        &failure_output[..failure_output.len().min(4_000)],
                    );
                }
            }
        }
    }

    // ── Architecture note ──────────────────────────────────────────────────
    // The retry loop above cold-restarts the backend on each attempt. It does
    // NOT maintain a persistent agent session across retries. Each attempt
    // launches a fresh backend process with accumulated failure context in the
    // task prompt. This is intentional — the current design prioritizes
    // simplicity and observability over session persistence. A future version
    // could keep the backend running (e.g., via a socket or API) and push
    // validation feedback into the existing conversation, but that would
    // require each backend to expose a continuation API. For now, the retry
    // loop is stateless: fail → append context → re-launch.
    //
    // The validation_commands list runs sequentially in the worktree directory.
    // All commands must exit 0 for the attempt to count as passing. The full
    // stdout+stderr of any failing command is fed back into the next attempt's
    // prompt, truncated to 8 000 chars to stay within context windows.
    // Because the backend is re-launched from scratch each attempt, the agent
    // must re-read the repo state — it cannot carry working memory between
    // attempts. This is acceptable for bounded code-generation tasks where
    // each attempt is self-contained.
    // ────────────────────────────────────────────────────────────────────────

    let has_changes = worktree::has_changes(&wt)?;
    if !has_changes {
        println!("No changes produced — nothing to push.");
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    let commit_msg = if validation_failed {
        format!(
            "gah: {} changes for {} [validation-failing draft]",
            args.mode, profile.repo_id
        )
    } else {
        format!("gah: {} changes for {}", args.mode, profile.repo_id)
    };

    println!("Changes detected. Committing and pushing...");
    let push_url = profile.push_url();
    worktree::commit_and_push_msg(&wt, &branch, &push_url, &commit_msg)?;

    let mr_title = if validation_failed {
        format!("[GAH][DRAFT-FAIL] {}: {}", args.mode, profile.repo_id)
    } else {
        format!("[GAH] {}: {}", args.mode, profile.repo_id)
    };
    let mr_body = format!(
        "## GAH {} mode\n\nBranch: `{}`\nTarget: `{}`\nValidation passed: {}\n\nGenerated by `gah dispatch`.",
        args.mode, branch, profile.default_target_branch, !validation_failed,
    );
    let mr = provider::create_draft_mr(profile, &branch, &mr_title, &mr_body)?;
    println!("Draft MR: {}", mr.url);

    worktree::cleanup(&wt, repo);
    Ok(())
}

fn pm(profile: &Profile, session_dir: &Path) -> Result<()> {
    let repo = Path::new(&profile.local_path);
    let log = git_output(&["log", "--oneline", "-20"], repo).unwrap_or_default();
    let test_count = count_test_files(profile, repo);
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
    let files = git_output(&["diff", "--name-only", &origin_ref, "HEAD"], repo).unwrap_or_default();

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

    let effective_backend = if args.backend == "auto" || args.backend.is_empty() {
        if which("claude").is_some() {
            "claude"
        } else {
            "openhands"
        }
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
            println!(
                "Run `claude -p \"$(cat {}/diff.patch)\"` to review manually.",
                bundle.display()
            );
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
                println!(
                    "OH profile:   {} (~/.openhands/profiles/{}.json)",
                    name, name
                );
                if let Some(m) = &args.model {
                    println!("Model override: {}", m);
                }
            } else {
                let cloud = args.backend == "cloud-coder";
                let default_model = cfg.defaults.llm_model(cloud);
                let model_name = args.model.as_deref().unwrap_or(&default_model);
                println!("LLM model:    {}", model_name);
                println!("LLM base:     {}", cfg.defaults.llm_base_url());
            }
            println!("Backend:      {}", args.backend);
            println!("Retries:      {}", args.retries);
            println!("Allow draft fail: {}", args.allow_draft_fail);
            if !profile.validation_commands.is_empty() {
                println!("Validation:");
                for cmd in &profile.validation_commands {
                    println!("  $ {}", cmd);
                }
            }
            if !args.target.is_empty() {
                let task_type = if Path::new(&args.target)
                    .extension()
                    .map_or(false, |e| e == "json")
                {
                    "candidate JSON"
                } else {
                    "task string"
                };
                println!("Task source:  {} ({})", args.target, task_type);
            }
            println!(
                "\nSteps: fetch → worktree → {} → [validate → retry]* → commit → push → draft MR",
                args.backend
            );
        }
        "pm" => println!("Steps: git log → test count → CI check → write pm-report.md"),
        "review" => println!("Steps: git diff → bundle → claude review"),
        other => println!("mode '{}': not yet implemented", other),
    }
    println!("\n## Safety\n- No pushes, no MRs, no provider calls (dry run)");
    Ok(())
}

/// Build the task prompt for the agent.
/// If `target` is a path to a candidates.json file, build a structured packet from the first candidate.
/// Otherwise use a generic prompt with target as a hint.
fn build_task(profile: &Profile, mode: &str, target: &str) -> String {
    // Try to load as candidate artifact
    if !target.is_empty() {
        let p = Path::new(target);
        if p.extension().map_or(false, |e| e == "json") && p.exists() {
            if let Ok(text) = fs::read_to_string(p) {
                if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                    if let Some(candidate) = artifact.candidates.first() {
                        return format_candidate_task(profile, candidate);
                    }
                }
            }
        }
    }

    // Generic prompt
    let mut task = format!(
        "Repository: {} ({})\n\
         Local path: {}\n\
         Target branch: {}\n\
         Mode: {}\n\n\
         Select the highest-priority unstarted task from the backlog, recent CI failures, or test gaps.\n\
         Make a small, focused change. Run tests if a test command exists. Do not push or create MRs.\n",
        profile.display_name, profile.repo, profile.local_path, profile.default_target_branch, mode,
    );
    if !target.is_empty() {
        task.push_str(&format!("\nFocus: {}\n", target));
    }
    task
}

fn format_candidate_task(profile: &Profile, c: &crate::models::Candidate) -> String {
    let mut out = format!(
        "# Task: {}\n\n\
         Repository: {} ({})\n\
         Local path: {}\n\
         Target branch: {}\n\n",
        c.candidate_id,
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
    );

    if !c.evidence.is_empty() {
        out.push_str("## Context\n");
        for e in &c.evidence {
            out.push_str(&format!("- {}\n", e));
        }
        out.push('\n');
    }

    if !c.affected_files.is_empty() {
        out.push_str("## Files likely involved\n");
        for f in &c.affected_files {
            out.push_str(&format!("- {}\n", f));
        }
        out.push('\n');
    }

    if !c.acceptance_criteria.is_empty() {
        out.push_str("## Acceptance criteria\n");
        for (i, ac) in c.acceptance_criteria.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, ac));
        }
        out.push('\n');
    }

    if !c.verification.is_empty() {
        out.push_str("## Verification steps\n");
        for (i, v) in c.verification.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, v));
        }
        out.push('\n');
    }

    out.push_str(
        "Make the changes required to satisfy the acceptance criteria above.\n\
         Run tests if a test command exists. Do not push or create MRs.\n",
    );
    out
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    worktree::git(args, cwd)
}

fn count_test_files(profile: &Profile, root: &Path) -> usize {
    let patterns = if profile.test_file_patterns.is_empty() {
        vec![
            "test_*.py".to_string(),
            "*_test.py".to_string(),
            "*.test.ts".to_string(),
            "*.test.js".to_string(),
            "*.spec.ts".to_string(),
            "*.spec.js".to_string(),
            "*_test.rs".to_string(),
            "tests/*.rs".to_string(),
            "*_test.go".to_string(),
            "*Test.java".to_string(),
            "*_spec.rb".to_string(),
            "*Tests.cs".to_string(),
        ]
    } else {
        profile.test_file_patterns.clone()
    };
    count_files_matching(root, root, &|name: &str| {
        patterns.iter().any(|pat| {
            let re = format!(
                "^{}$",
                pat.replace(".", r"\.").replace("*", ".*").replace("?", ".")
            );
            regex::Regex::new(&re)
                .map(|r| r.is_match(name))
                .unwrap_or(false)
        })
    })
}

fn count_files_matching(root: &Path, dir: &Path, pred: &dyn Fn(&str) -> bool) -> usize {
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
                count += count_files_matching(root, &path, pred);
            }
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if pred(&rel.to_string_lossy()) {
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
