use crate::config::{self, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::models::CandidateArtifact;
use crate::models::{PmPlan, PmPlanTicket};
use crate::routing::{self, RouteDecision, RouteError, RouteRequest};
use crate::{provider, runner, usage, worktree};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub struct DispatchArgs {
    pub profile: String,
    pub mode: String,
    pub backend: String,
    pub target: String,
    pub branch: Option<String>,
    pub mr: Option<String>,
    pub current_branch: bool,
    /// Reserved for future per-run cost/turn budget enforcement; not yet read.
    #[allow(dead_code)]
    pub budget: u32,
    pub dry_run: bool,
    /// Already consumed by the caller to load `cfg`; kept on the struct for CLI plumbing symmetry.
    #[allow(dead_code)]
    pub config_path: Option<String>,
    pub oh_profile: Option<String>,
    pub model: Option<String>,
    pub retries: u32,
    pub allow_draft_fail: bool,
    /// Require explicit --prod flag to load production env_file_prod.
    /// Without this flag, only env_file (dev) is loaded.
    pub prod: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationFailureProgress {
    Changed,
    UnchangedFromBaseline,
    UnchangedFromPreviousAttempt,
    UnchangedFromBaselineAndPreviousAttempt,
}

impl ValidationFailureProgress {
    fn unchanged_from_baseline(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromBaseline | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }

    fn unchanged_from_previous_attempt(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromPreviousAttempt | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }
}

fn validation_failure_no_progress_reason(
    progress: ValidationFailureProgress,
) -> Option<&'static str> {
    match progress {
        ValidationFailureProgress::Changed => None,
        ValidationFailureProgress::UnchangedFromBaseline => Some(
            "validation failure identical to the pristine-tree baseline — the agent's changes never affected this error. Fix the validation command or environment, not the ticket.",
        ),
        ValidationFailureProgress::UnchangedFromPreviousAttempt => Some(
            "validation failure identical to the previous attempt — the agent made no progress on the failing check.",
        ),
        ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt => Some(
            "validation failure identical to both the pristine-tree baseline and the previous attempt — the agent made no progress and never affected the original error.",
        ),
    }
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
    let mut ledger = LedgerEntry::new(
        &args.profile,
        profile,
        &args.backend,
        &args.mode,
        &args.target,
        Some(ts.clone()),
        Some(&session_dir),
    );
    let started = Instant::now();
    fs::create_dir_all(&session_dir)?;
    println!("Session: {}", session_dir.display());

    let result = match args.mode.as_str() {
        "improve" | "fix" => improve(cfg, profile, args, &session_dir, &mut ledger),
        "pm" => pm(cfg, profile, args, &session_dir, &mut ledger),
        "review" => review(cfg, profile, args, &session_dir, &mut ledger),
        "experiment" => experiment(cfg, profile, args, &session_dir, &mut ledger),
        other => anyhow::bail!("unknown mode: {}", other),
    };
    ledger.duration_seconds = Some(started.elapsed().as_secs_f64());
    if let Err(err) = &result {
        ledger.error_summary = Some(summarize_error(err));
    }
    if let Err(err) = crate::ledger::append(cfg, &ledger) {
        eprintln!("warning: failed to append ledger entry: {:#}", err);
    }
    result
}

fn resolve_llm(
    cfg: &GahConfig,
    args: &DispatchArgs,
    profile_oh: Option<&str>,
    effective_model: Option<&str>,
) -> Result<runner::LlmConfig> {
    // CLI flag wins, then profile config, then default
    let effective_oh_profile = args.oh_profile.as_deref().or(profile_oh);
    if let Some(name) = effective_oh_profile {
        let mut llm = runner::load_oh_profile(name)?;
        if let Some(m) = &args.model {
            llm.model = m.clone();
        }
        if let Some(m) = effective_model {
            llm.model = m.to_string();
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
    // --model flag always wins
    if let Some(m) = &args.model {
        return Ok(runner::LlmConfig {
            base_url: cfg.defaults.llm_base_url(),
            api_key: cfg.defaults.llm_api_key(),
            model: m.clone(),
        });
    }
    if let Some(m) = effective_model {
        return Ok(runner::LlmConfig {
            base_url: cfg.defaults.llm_base_url(),
            api_key: cfg.defaults.llm_api_key(),
            model: m.to_string(),
        });
    }
    // Check profile-level mode-specific override, then global default
    let profile_model =
        config::get_profile(cfg, &args.profile)
            .ok()
            .and_then(|p| match args.mode.as_str() {
                "improve" | "fix" => p.model_improve.clone(),
                "pm" => p.model_pm.clone(),
                "review" => p.model_review.clone(),
                _ => None,
            });
    let cloud = args.backend == "cloud-coder";
    Ok(runner::LlmConfig {
        base_url: cfg.defaults.llm_base_url(),
        api_key: cfg.defaults.llm_api_key(),
        model: profile_model.unwrap_or_else(|| cfg.defaults.llm_model(cloud)),
    })
}

#[allow(clippy::too_many_arguments)]
fn run_backend(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
    effective_model: Option<&str>,
    env_path: Option<&str>,
) -> Result<runner::RunResult> {
    let env_vars = env_path.map(runner::load_env_file).unwrap_or_default();
    match backend {
        "codex" => runner::run_codex_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            effective_model,
            &profile.codex_args,
            &env_vars,
        ),
        "claude" => runner::run_claude_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            &profile.claude_args,
            &env_vars,
        ),
        "agy" | "agy-main" | "agy-second" => runner::run_agy_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            llm,
            &env_vars,
        ),
        _ => runner::run_openhands(
            wt,
            task,
            session_dir,
            llm,
            &profile.openhands_args,
            &env_vars,
        ),
    }
}

/// Run validation_commands in the worktree. Returns Err(combined output) on first failure.
fn validate(commands: &[String], wt: &Path) -> Result<()> {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        println!("  Validating: {}", cmd_str);
        // Run through the shell: validation commands routinely use `cd x && y`,
        // pipes, and env vars, which Command::new(bin) cannot execute.
        let out = Command::new("sh")
            .args(["-c", cmd_str])
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

fn preflight(profile: &Profile, backend: &str) -> Result<()> {
    ensure_bin("git")?;
    runner::require_backend_executable(profile, backend)?;
    Ok(())
}

fn ensure_bin(bin: &str) -> Result<()> {
    if which(bin).is_some() {
        Ok(())
    } else {
        anyhow::bail!("required binary '{}' not found on PATH", bin);
    }
}

fn command_output(bin: &str, args: &[&str], cwd: &Path) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("{} {}", bin, args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "{} {}: {}",
            bin,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Check profile policy before provisioning any worktree.
/// If a policy_path is set, the requested action must be allowed or dispatch
/// hard-fails before any mutations occur.
fn enforce_policy(profile: &Profile, action: &str) -> Result<()> {
    let Some(policy_path) = &profile.policy_path else {
        return Ok(()); // no policy file = trust the user
    };
    let text = std::fs::read_to_string(policy_path)
        .with_context(|| format!("reading policy file: {}", policy_path))?;
    let cfg: crate::models::PolicyConfig =
        toml::from_str(&text).with_context(|| format!("parsing policy file: {}", policy_path))?;
    let repo = cfg.repo;
    let allowed = match repo.trust_mode.as_str() {
        "read_only" => false,
        "draft_pr_allowed" => match action {
            "open-draft-pr" => {
                repo.allow_provider_mutation && repo.allow_push && repo.allow_draft_pr
            }
            "edit-issue" => repo.allow_issue_write,
            "git-push" => repo.allow_push,
            "git-push-prod" => repo.allow_project_write,
            _ => false,
        },
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        anyhow::bail!(
            "POLICY BLOCKED: trust_mode={:?} does not allow action={:?}.              Set allow_push/allow_draft_pr/allow_project_write in {} or              pass --override-policy if you know what you're doing.",
            repo.trust_mode, action, policy_path
        )
    }
}

fn improve(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    // Enforce policy before any mutations
    let push_action = if args.prod {
        "git-push-prod"
    } else {
        "git-push"
    };
    enforce_policy(profile, "open-draft-pr")?;
    enforce_policy(profile, push_action)?;

    let target = if args.target.is_empty() {
        let default = PathBuf::from(&profile.artifact_root)
            .join("candidates")
            .join("latest.json");
        if default.exists() {
            println!("Auto-target: {}", default.display());
            default.to_string_lossy().into_owned()
        } else {
            args.target.clone()
        }
    } else {
        args.target.clone()
    };
    let ticket_meta = parse_ticket_metadata(Path::new(&target)).ok().flatten();
    let usage_summary = ledger::usage_summary_for_backend(
        cfg,
        args.backend.as_str(),
        args.model.as_deref(),
        Some(
            session_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ),
    )
    .ok();
    let route_req = RouteRequest {
        mode: &args.mode,
        requested_backend: &args.backend,
        requested_model: args.model.as_deref(),
        recommended_backend: ticket_meta
            .as_ref()
            .and_then(|m| m.recommended_backend.as_deref()),
        recommended_model: ticket_meta
            .as_ref()
            .and_then(|m| m.recommended_model.as_deref()),
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary,
    };
    let mut route = decide_route(cfg, profile, route_req.clone(), ledger)?;
    apply_route_to_ledger(ledger, &route);
    preflight(profile, &route.effective_backend)?;
    let mut llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        route.effective_model.as_deref(),
    )?;

    // Resolve env_file: use env_file_prod if --prod, otherwise env_file (dev)
    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  \u{26a0}\u{fe0f}  PRODUCTION env - agent has live API access");
        }
    }

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
    ledger.branch = Some(branch.clone());
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);

    let mut base_task = build_task(profile, &wt, &args.mode, &target);

    // Baseline: run validation once on the pristine worktree BEFORE spending
    // tokens. A failure here is a config error or a pre-existing red repo —
    // either way the agent must know, and later failures can be compared
    // against it to tell "agent made no progress" from "agent broke it".
    let baseline_failure = if profile.validation_commands.is_empty() {
        None
    } else {
        println!("Baseline validation on pristine worktree...");
        match validate(&profile.validation_commands, &wt) {
            Ok(()) => {
                println!("Baseline validation passed.");
                None
            }
            Err(e) => Some(format!("{:#}", e)),
        }
    };
    if let Some(b) = &baseline_failure {
        fs::write(session_dir.join("baseline-validation-failure.txt"), b)?;
        println!("Baseline validation ALREADY FAILING on untouched branch (recorded).");
        base_task.push_str(&format!(
            "\n\n## Warning: validation already fails on the untouched branch\n\n```\n{}\n```\n\nIf this ticket is about fixing that failure, fix it. Otherwise it is pre-existing — your changes must not add new failures.\n",
            &b[..b.len().min(4_000)],
        ));
    }

    let mut task = base_task.clone();
    let max_attempts = args.retries + 1;
    let mut validation_failed = false;
    let mut prev_failure: Option<String> = None;
    for attempt in 0..max_attempts {
        println!(
            "\nAttempt {}/{}: running {} backend...",
            attempt + 1,
            max_attempts,
            route.effective_backend
        );
        let attempt_session = session_dir.join(format!("attempt-{}", attempt + 1));
        fs::create_dir_all(&attempt_session)?;
        ledger.attempts_started += 1;
        let attempt_start = std::time::Instant::now();

        let env_path = if !resolved_env.is_empty() {
            Some(resolved_env)
        } else {
            None
        };
        let result = run_backend(
            &route.effective_backend,
            profile,
            &wt,
            &task,
            &attempt_session,
            &llm,
            route.effective_model.as_deref(),
            env_path,
        );
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                // The backend process itself couldn't launch (binary missing,
                // exec failure) — this is a setup/harness problem, not the
                // agent or backend failing at its job.
                ledger.set_failure(
                    crate::ledger::FailureClass::HarnessError,
                    crate::ledger::FailureStage::BackendLaunch,
                );
                ledger.attempts.push(crate::ledger::AttemptRecord {
                    attempt_number: attempt + 1,
                    backend: route.effective_backend.clone(),
                    effective_model: Some(llm.model.clone()),
                    exit_code: None,
                    validation_result: None,
                    failure_class: Some(crate::ledger::FailureClass::HarnessError.as_str().into()),
                    failure_stage: Some(crate::ledger::FailureStage::BackendLaunch.as_str().into()),
                    duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                    diff_path: None,
                });
                worktree::cleanup(&wt, repo);
                return Err(e);
            }
        };
        // The backend process launched and ran to an exit code, regardless
        // of what that code was — "completed" tracks whether the attempt
        // got a fair shot, not whether it succeeded.
        ledger.attempts_completed += 1;

        println!(
            "Backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);

        if result.exit_code != 0 {
            // The backend launched but exited nonzero — the backend itself
            // failed at its job, distinct from it never starting at all.
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::AgentRun,
            );
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(result.exit_code),
                validation_result: None,
                failure_class: Some(crate::ledger::FailureClass::BackendError.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
            });
            let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
            if attempt + 1 < max_attempts {
                if let Some(parsed) = mark_backend_unavailable_from_output(
                    &route.effective_backend,
                    route.effective_model.as_deref(),
                    &log_text,
                    &result.log_path,
                )? {
                    let rerouted = decide_route(cfg, profile, route_req.clone(), ledger)?;
                    let current_identity =
                        route_identity(&route.effective_backend, route.effective_model.as_deref());
                    let rerouted_identity = route_identity(
                        &rerouted.effective_backend,
                        rerouted.effective_model.as_deref(),
                    );
                    if rerouted_identity != current_identity {
                        println!(
                            "Backend unavailable; retrying next attempt with {} instead of {} ({:?})",
                            rerouted.effective_backend, route.effective_backend, parsed.kind
                        );
                        route = rerouted;
                        apply_route_to_ledger(ledger, &route);
                        preflight(profile, &route.effective_backend)?;
                        llm = resolve_llm(
                            cfg,
                            args,
                            profile.oh_profile.as_deref(),
                            route.effective_model.as_deref(),
                        )?;
                        continue;
                    }
                }
            }
            worktree::cleanup(&wt, repo);
            anyhow::bail!(
                "backend exited {} on attempt {}",
                result.exit_code,
                attempt + 1
            );
        }

        if profile.validation_commands.is_empty() {
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(0),
                validation_result: None,
                failure_class: None,
                failure_stage: None,
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
            });
            break;
        }

        println!(
            "Running validation ({} commands)...",
            profile.validation_commands.len()
        );
        match validate(&profile.validation_commands, &wt) {
            Ok(()) => {
                println!("Validation passed.");
                validation_failed = false;
                ledger.validation_result = Some("passed".into());
                ledger.attempts.push(crate::ledger::AttemptRecord {
                    attempt_number: attempt + 1,
                    backend: route.effective_backend.clone(),
                    effective_model: Some(llm.model.clone()),
                    exit_code: Some(0),
                    validation_result: Some("passed".into()),
                    failure_class: None,
                    failure_stage: None,
                    duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                    diff_path: None,
                });
                break;
            }
            Err(e) => {
                validation_failed = true;
                let failure_output = format!("{:#}", e);
                let failure_path = attempt_session.join("validation-failure.txt");
                fs::write(&failure_path, &failure_output)?;
                println!("Validation failed ({})", failure_path.display());

                // Identical failure to the previous attempt means the agent's
                // changes had no effect on the error — almost always an
                // environment/config problem the agent cannot fix. Stop burning
                // attempts.
                let failure_progress = classify_validation_failure_progress(
                    baseline_failure.as_deref(),
                    prev_failure.as_deref(),
                    &failure_output,
                );
                prev_failure = Some(failure_output.clone());

                if attempt + 1 < max_attempts
                    && !failure_progress.unchanged_from_baseline()
                    && !failure_progress.unchanged_from_previous_attempt()
                {
                    // Save the failed attempt's diff before wiping, so the
                    // session artifact shows what the agent actually wrote.
                    let _ = worktree::git(&["add", "-A"], &wt);
                    let mut diff_path = None;
                    if let Ok(diff) = worktree::git(&["diff", "--cached"], &wt) {
                        let path = attempt_session.join("attempt-diff.patch");
                        if fs::write(&path, diff).is_ok() {
                            diff_path = Some(path.display().to_string());
                        }
                    }
                    // Wipe bad code so next attempt starts clean
                    let _ = worktree::git(&["reset", "--hard", "HEAD"], &wt);
                    let _ = worktree::git(&["clean", "-fd"], &wt);
                    println!("Retrying with failure context...");
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::ValidationFailure
                                .as_str()
                                .into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path,
                    });
                    // Rebuild from the base task with only the latest failure —
                    // accumulating retry blocks confuses smaller models.
                    task = format!(
                        "{}\n\n## Previous attempt failed validation (attempt {}/{})\n\nYour previous attempt was discarded. The worktree is clean again.\nFix the following before completing the task:\n\n```\n{}\n```",
                        base_task,
                        attempt + 1,
                        max_attempts,
                        &failure_output[..failure_output.len().min(8_000)],
                    );
                } else if attempt + 1 < max_attempts && !args.allow_draft_fail {
                    let Some(reason) = validation_failure_no_progress_reason(failure_progress)
                    else {
                        worktree::cleanup(&wt, repo);
                        anyhow::bail!(
                            "validation failed after {} attempt(s). Use --allow-draft-fail to push anyway.\n\n{}",
                            max_attempts,
                            &failure_output[..failure_output.len().min(4_000)],
                        );
                    };
                    // Identical to baseline and/or the previous attempt: the
                    // agent made no measurable progress, which is a distinct
                    // failure mode from "tried and failed differently."
                    ledger.set_failure(
                        crate::ledger::FailureClass::AgentNoProgress,
                        crate::ledger::FailureStage::PostValidation,
                    );
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::AgentNoProgress.as_str().into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                    });
                    worktree::cleanup(&wt, repo);
                    anyhow::bail!(
                        "{} Aborting early after attempt {}.\n\n{}",
                        reason,
                        attempt + 1,
                        &failure_output[..failure_output.len().min(4_000)],
                    );
                } else if args.allow_draft_fail {
                    println!(
                        "Validation still failing; --allow-draft-fail set — pushing as draft."
                    );
                    ledger.validation_result = Some("failed-draft".into());
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed-draft".into()),
                        failure_class: None,
                        failure_stage: None,
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                    });
                    break;
                } else {
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::ValidationFailure
                                .as_str()
                                .into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                    });
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

    if profile.validation_commands.is_empty() && ledger.validation_result.is_none() {
        ledger.validation_result = Some("not_run".into());
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

    let has_changes = worktree::has_changes(&wt, &profile.default_target_branch)?;
    if !has_changes {
        println!("No changes produced — nothing to push.");
        worktree::cleanup(&wt, repo);
        return Ok(());
    }
    apply_diff_stats(ledger, &wt, &profile.default_target_branch);

    let commit_msg = if validation_failed {
        format!(
            "gah: {} changes for {} [validation-failing draft]",
            args.mode, profile.repo_id
        )
    } else {
        format!("gah: {} changes for {}", args.mode, profile.repo_id)
    };

    println!("Changes detected. Committing and pushing...");
    let push_url = profile.push_url()?;
    let push_pat = profile.pat();
    ledger.commit_attempted = true;
    worktree::stage_all(&wt)?;
    worktree::ensure_staged(&wt)?;
    worktree::commit_msg(&wt, &commit_msg)?;
    ledger.commit_created = true;
    ledger.push_attempted = true;
    worktree::push_branch(&wt, &branch, &push_url, &push_pat)?;
    ledger.push_succeeded = true;

    let mr_title = build_mr_title(
        &args.mode,
        &profile.repo_id,
        validation_failed,
        ticket_meta.as_ref(),
    );
    let mr_body = format!(
        "## GAH {} mode\n\nTicket: {}\nBackend/model: `{}` / `{}`\nBranch: `{}`\nTarget: `{}`\nValidation passed: {}\n\nGenerated by `gah dispatch`.",
        args.mode,
        render_ticket_label(ticket_meta.as_ref()),
        route.effective_backend,
        llm.model,
        branch,
        profile.default_target_branch,
        !validation_failed,
    );
    ledger.mr_attempted = true;
    let mr = provider::create_draft_mr(profile, &branch, &mr_title, &mr_body)?;
    ledger.mr_created = true;
    ledger.mr_url = Some(mr.url.clone());
    println!("Draft MR: {}", mr.url);

    worktree::cleanup(&wt, repo);
    Ok(())
}

fn experiment(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let route = decide_route(
        cfg,
        profile,
        RouteRequest {
            mode: "experiment",
            requested_backend: &args.backend,
            requested_model: args.model.as_deref(),
            recommended_backend: None,
            recommended_model: None,
            session_id: session_dir.file_name().and_then(|s| s.to_str()),
            usage_summary: None,
        },
        ledger,
    )?;
    apply_route_to_ledger(ledger, &route);
    preflight(profile, &route.effective_backend)?;
    let llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        route.effective_model.as_deref(),
    )?;

    // Resolve env_file: use env_file_prod if --prod, otherwise env_file (dev)
    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  \u{26a0}\u{fe0f}  PRODUCTION env - agent has live API access");
        }
    }

    let ts = timestamp();
    let branch = format!("gah/exp-{}-{}", profile.repo_id, &ts);
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
    ledger.branch = Some(branch.clone());
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);

    let task = build_task(profile, &wt, "experiment", &args.target);
    let attempt_dir = session_dir.join("attempt-1");
    fs::create_dir_all(&attempt_dir)?;

    let env_path = if !resolved_env.is_empty() {
        Some(resolved_env)
    } else {
        None
    };
    let result = match run_backend(
        &route.effective_backend,
        profile,
        &wt,
        &task,
        &attempt_dir,
        &llm,
        route.effective_model.as_deref(),
        env_path,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Backend error (continuing for judge evaluation): {:#}", e);
            let log_path = attempt_dir.join("backend-output.log");
            let _ = std::fs::write(&log_path, format!("Backend error: {:#}", e));
            runner::RunResult {
                exit_code: -1,
                duration_secs: 0.0,
                log_path: log_path.to_string_lossy().into_owned(),
            }
        }
    };
    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );
    ledger.backend_exit_code = Some(result.exit_code);

    // Collect research artifacts (notebooks, plots, CSVs, reports)
    let artifacts_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir)?;
    let artifact_count = collect_artifacts(&wt, &artifacts_dir);
    println!("Artifacts collected: {}", artifact_count);

    // Judge whether the task was answered
    let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
    let answered = judge_experiment(&args.target, &log_text, artifact_count);
    println!(
        "Judge: {}",
        if answered {
            "ANSWERED"
        } else {
            "PARTIAL/UNANSWERED"
        }
    );

    if !answered && artifact_count == 0 {
        println!(
            "No artifacts and judge says task not answered. \
             Check {} for agent output.",
            result.log_path
        );
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    let has_changes = worktree::has_changes(&wt, &profile.default_target_branch)?;
    if !has_changes {
        println!(
            "No code changes in worktree — artifacts saved to {}",
            artifacts_dir.display()
        );
        worktree::cleanup(&wt, repo);
        return Ok(());
    }
    ledger.validation_result = Some(if answered {
        "answered".into()
    } else {
        "partial".into()
    });
    apply_diff_stats(ledger, &wt, &profile.default_target_branch);

    println!("Changes detected. Committing and pushing...");
    let commit_msg = format!("gah: experiment for {}", profile.repo_id);
    let push_url = profile.push_url()?;
    let push_pat = profile.pat();
    ledger.commit_attempted = true;
    worktree::stage_all(&wt)?;
    worktree::ensure_staged(&wt)?;
    worktree::commit_msg(&wt, &commit_msg)?;
    ledger.commit_created = true;
    ledger.push_attempted = true;
    worktree::push_branch(&wt, &branch, &push_url, &push_pat)?;
    ledger.push_succeeded = true;

    let mr_body = format!(
        "## GAH Experiment\n\nBackend/model: `{}` / `{}`\nBranch: `{}`\nTarget: `{}`\n\
         Artifacts: {}\nJudge verdict: {}\n\n\
         Generated by `gah dispatch --mode experiment`.",
        route.effective_backend,
        llm.model,
        branch,
        profile.default_target_branch,
        artifact_count,
        if answered { "ANSWERED" } else { "PARTIAL" },
    );
    ledger.mr_attempted = true;
    let mr = provider::create_draft_mr(
        profile,
        &branch,
        &format!("[GAH][EXP] {}", profile.repo_id),
        &mr_body,
    )?;
    ledger.mr_created = true;
    ledger.mr_url = Some(mr.url.clone());
    println!("Draft MR: {}", mr.url);

    worktree::cleanup(&wt, repo);
    Ok(())
}

/// Copy untracked artifact files (notebooks, plots, data exports) to out_dir.
fn collect_artifacts(wt: &Path, out: &Path) -> usize {
    const ARTIFACT_EXTS: &[&str] = &["ipynb", "html", "png", "jpg", "jpeg", "csv", "parquet"];

    // Intentionally omit --exclude-standard: ML repos gitignore *.csv, *.png,
    // *.parquet etc. to avoid committing large datasets. We want those files.
    let Ok(output) = Command::new("git")
        .args(["ls-files", "--others"])
        .current_dir(wt)
        .output()
    else {
        return 0;
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut count = 0;
    for line in text.lines() {
        let path = wt.join(line.trim());
        let is_artifact = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| ARTIFACT_EXTS.contains(&e))
            .unwrap_or(false);
        if is_artifact {
            if let Some(name) = path.file_name() {
                let _ = fs::copy(&path, out.join(name));
                count += 1;
            }
        }
    }
    count
}

/// Ask an LLM judge whether the agent answered the task.
/// Always calls Claude regardless of artifact count — a blank file is not success.
/// Falls back to artifact presence only when `claude` is unavailable.
fn judge_experiment(task: &str, log: &str, artifact_count: usize) -> bool {
    let artifact_note = if artifact_count > 0 {
        format!("{} output file(s) were produced.", artifact_count)
    } else {
        "No output files were produced.".to_string()
    };
    let prompt = format!(
        "You are evaluating whether an AI agent meaningfully answered a research task.\n\n\
         Task:\n{}\n\n\
         Agent output (last 3000 chars):\n{}\n\n\
         {}\n\n\
         Did the agent produce a substantive, non-trivial answer to the task? \
         An empty file, a stub, or a generic error message does not count. \
         Reply with only YES or NO.",
        &task[..task.len().min(500)],
        &log[log.len().saturating_sub(3000)..],
        artifact_note,
    );
    Command::new("claude")
        .args(["-p", &prompt])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_uppercase()
                .contains("YES")
        })
        // claude unavailable: fall back to artifact presence as a weak signal
        .unwrap_or(artifact_count > 0)
}

fn pm(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    // Without a target: static repo snapshot (context for the agent, not a dispatch)
    if args.target.is_empty() {
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
            "# PM Report: {}\n\nRepo: {}\nBranch: {}\nTest files: {}\nCI configured: {}\n\n\
             ## Recent commits\n```\n{}\n```\n\n## README\n{}\n",
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
        ledger.validation_result = Some("not_run".into());
        return Ok(());
    }

    // With a target: dispatch an LLM to produce a structured ticket plan.
    let preflight_ctx = collect_pm_preflight(profile, repo)?;
    let route_req = RouteRequest {
        mode: "pm",
        requested_backend: &args.backend,
        requested_model: args.model.as_deref(),
        recommended_backend: None,
        recommended_model: None,
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary: None,
    };
    let mut plan_route = decide_route(cfg, profile, route_req.clone(), ledger)?;
    apply_route_to_ledger(ledger, &plan_route);
    preflight(profile, &plan_route.effective_backend)?;
    let mut llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        plan_route.effective_model.as_deref(),
    )?;

    // Resolve env_file: use env_file_prod if --prod, otherwise env_file (dev)
    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  \u{26a0}\u{fe0f}  PRODUCTION env - agent has live API access");
        }
    }

    let task = build_pm_plan_task(profile, &preflight_ctx, &args.target)?;

    let mut attempted_routes = HashSet::new();
    let log_text = loop {
        let attempt_index = attempted_routes.len() + 1;
        let attempt_dir = session_dir.join(format!("pm-run-{attempt_index}"));
        fs::create_dir_all(&attempt_dir)?;
        fs::write(attempt_dir.join("task.md"), &task)?;

        let result = run_backend(
            &plan_route.effective_backend,
            profile,
            repo,
            &task,
            &attempt_dir,
            &llm,
            plan_route.effective_model.as_deref(),
            None,
        )?;
        println!(
            "PM backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);
        ledger.validation_result = Some("not_run".into());

        let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
        if result.exit_code == 0 {
            break log_text;
        }

        ledger.set_failure(
            crate::ledger::FailureClass::BackendError,
            crate::ledger::FailureStage::AgentRun,
        );
        let route_key = route_identity(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
        );
        attempted_routes.insert(route_key);
        let parsed = mark_backend_unavailable_from_output(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
            &log_text,
            &result.log_path,
        )?;
        let Some(parsed) = parsed else {
            anyhow::bail!(
                "PM backend exited {} with no recognized quota/rate-limit signal in {}",
                result.exit_code,
                result.log_path
            );
        };

        let rerouted = decide_route(cfg, profile, route_req.clone(), ledger)?;
        let rerouted_key = route_identity(
            &rerouted.effective_backend,
            rerouted.effective_model.as_deref(),
        );
        if attempted_routes.contains(&rerouted_key) {
            anyhow::bail!(
                "PM backend {} became unavailable ({:?}) and no new eligible backend remained",
                plan_route.effective_backend,
                parsed.kind
            );
        }
        println!(
            "PM rerouting: {} -> {} ({:?})",
            plan_route.effective_backend, rerouted.effective_backend, parsed.kind
        );
        plan_route = rerouted;
        apply_route_to_ledger(ledger, &plan_route);
        preflight(profile, &plan_route.effective_backend)?;
        llm = resolve_llm(
            cfg,
            args,
            profile.oh_profile.as_deref(),
            plan_route.effective_model.as_deref(),
        )?;
    };

    let plan = parse_pm_plan(&log_text)?;
    let written = apply_pm_plan(repo, &preflight_ctx, &plan)?;
    println!("\nCreated {} ticket(s):", written.len());
    for path in &written {
        println!("  {}", path.display());
    }

    Ok(())
}

fn build_pm_plan_task(profile: &Profile, ctx: &PmPreflight, target: &str) -> Result<String> {
    Ok(format!(
        "Repository: {} ({})\nLocal path: {}\nTarget branch: {}\n\n\
         You are a project manager. Use only the preflight context below plus the target request. \
         Do not assume you will remember to inspect the repo yourself.\n\
         Do not write code in PM mode.\n\
         Return only valid JSON matching this schema:\n\
         {{\"title\":string,\"summary\":string,\"tickets\":[{{\"title\":string,\"summary\":string,\"difficulty\":\"easy|medium|hard\",\"risk\":\"low|medium|high\",\"recommended_backend\":string|null,\"duplicate_evidence\":[string],\"affected_files\":[string],\"acceptance_criteria\":[string],\"verification_commands\":[string],\"uncovered_reason\":string}}]}}\n\n\
         Rules:\n\
         - Default action is to avoid creating new tickets.\n\
         - Do not create a ticket if an open MR already covers the issue.\n\
         - Do not create a ticket if an existing ticket already covers the issue.\n\
         - Do not create a ticket if a recently merged MR already fixed the issue.\n\
         - Consolidate small related fixes into one ticket.\n\
         - Prefer \"already covered\" or \"update existing ticket\" when unsure.\n\
         - If creating tickets, create only genuinely uncovered, atomic work.\n\
         - Cite the relevant MR or ticket evidence whenever you decide work is already covered.\n\
         - If the work is already covered, return an empty tickets array.\n\
         - Each new ticket must be independently completable in one session.\n\n\
         ## Preflight Context\n\n{}\n\n\
         ## Target Request\n\n{}\n",
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
        ctx.rendered,
        target,
    ))
}

fn collect_pm_preflight(profile: &Profile, repo: &Path) -> Result<PmPreflight> {
    let memory_path = repo.join("docs/MANAGER_MEMORY.md");
    let manager_memory = fs::read_to_string(&memory_path).with_context(|| {
        format!(
            "PM mode requires manager memory at {}",
            memory_path.display()
        )
    })?;

    let repo_state = collect_pm_repo_state(repo);
    let tickets = collect_ticket_summaries(&repo.join("docs/tickets"))?;
    let open_mrs = collect_mr_context(profile, repo, "opened", None)?;
    let merged_mrs = collect_mr_context(profile, repo, "merged", Some("20"))?;

    let mut out = String::new();
    out.push_str("### Manager Memory\n");
    out.push_str(&manager_memory);
    if !manager_memory.ends_with('\n') {
        out.push('\n');
    }

    out.push_str("\n### Open Merge Requests\n");
    out.push_str(&open_mrs);
    out.push_str("\n\n### Recently Merged Merge Requests\n");
    out.push_str(&merged_mrs);
    out.push_str("\n\n### Existing Tickets\n");
    if tickets.is_empty() {
        out.push_str("(none found)");
    } else {
        out.push_str(&tickets.join("\n"));
    }
    out.push_str("\n\n### Repo State\n");
    out.push_str(&repo_state);
    Ok(PmPreflight {
        rendered: out,
        existing_tickets: tickets,
        open_mrs,
        merged_mrs,
    })
}

fn collect_mr_context(
    profile: &Profile,
    repo: &Path,
    state: &str,
    per_page: Option<&str>,
) -> Result<String> {
    if profile.provider != "gitlab" {
        return Ok("(provider is not gitlab)".to_string());
    }

    ensure_bin("glab")?;
    let mut args = vec![
        "mr",
        "list",
        "--repo",
        profile.repo.as_str(),
        "--state",
        state,
    ];
    if let Some(per_page) = per_page {
        args.push("--per-page");
        args.push(per_page);
    }
    let output = command_output("glab", &args, repo)?;
    if output.is_empty() {
        Ok("(none)".to_string())
    } else {
        Ok(output)
    }
}

fn collect_ticket_summaries(tickets_dir: &Path) -> Result<Vec<String>> {
    if !tickets_dir.exists() {
        return Ok(vec![]);
    }

    let mut tickets = vec![];
    for entry in fs::read_dir(tickets_dir)
        .with_context(|| format!("reading {}", tickets_dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = fs::read_to_string(&path).unwrap_or_default();
        let title = first_markdown_heading(&body).unwrap_or("(no heading)");
        tickets.push(format!(
            "- {}: {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            title
        ));
    }
    tickets.sort();
    Ok(tickets)
}

fn first_markdown_heading(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find_map(|line| {
        if !line.starts_with('#') {
            return None;
        }
        let stripped = line.trim_start_matches('#').trim();
        (!stripped.is_empty()).then_some(stripped)
    })
}

fn collect_pm_repo_state(repo: &Path) -> String {
    let branch = command_output("git", &["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let dirty = command_output("git", &["status", "--short"], repo)
        .map(|s| if s.is_empty() { "clean".to_string() } else { s })
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let commits = command_output("git", &["log", "--oneline", "-5"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));

    format!(
        "Current branch: {}\n\nDirty status:\n{}\n\nRecent commits:\n{}",
        branch, dirty, commits
    )
}

fn review(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let repo = Path::new(&profile.local_path);
    let route = decide_route(
        cfg,
        profile,
        RouteRequest {
            mode: "review",
            requested_backend: &args.backend,
            requested_model: args.model.as_deref(),
            recommended_backend: None,
            recommended_model: None,
            session_id: session_dir.file_name().and_then(|s| s.to_str()),
            usage_summary: None,
        },
        ledger,
    )?;
    apply_route_to_ledger(ledger, &route);
    let mut target = resolve_review_target(cfg, profile, args)?;
    if target.prior_state.is_none() {
        target.prior_state =
            lookup_review_state_by_branch(cfg, &args.profile, &target.source_branch);
    }
    let diff_bundle = prepare_review_diff(repo, profile, &target)?;

    let bundle = session_dir.join("review-bundle");
    fs::create_dir_all(&bundle)?;
    fs::write(bundle.join("diff.patch"), &diff_bundle.diff)?;
    fs::write(bundle.join("changed-files.txt"), &diff_bundle.files)?;
    fs::write(
        bundle.join("mr-description.md"),
        format!(
            "MR: {}\nURL: {}\nSource: {}\nTarget: {}\nRepo: {}\nTitle: {}\nCI: {}",
            target.mr_id.as_deref().unwrap_or("n/a"),
            target.mr_url.as_deref().unwrap_or("n/a"),
            target.source_branch,
            target.target_branch,
            profile.repo,
            target.mr_title.as_deref().unwrap_or("n/a"),
            target.ci_status.as_deref().unwrap_or("unknown"),
        ),
    )?;

    println!(
        "Diff: {} bytes, files: {}",
        diff_bundle.diff.len(),
        diff_bundle.files.lines().count()
    );

    let prompt = format!(
        "Review this diff for correctness, test coverage, and safety. \
         Return two sections:\n\
         1. Markdown review notes.\n\
         2. A JSON object with fields: verdict, confidence, human_required, blocking_findings, non_blocking_findings, risk_notes.\n\
         blocking_findings, non_blocking_findings, and risk_notes must be JSON arrays of strings, even when empty or when only one item exists.\n\
         Verdict must be one of APPROVE_STRONG, APPROVE_WEAK, NEEDS_FIX, REJECT, HUMAN_REVIEW.\n\
         Repo: {}. MR: {}. Source: {}. Target: {}. CI status: {}.\n\
         MR title: {}\nMR body:\n{}\n\
         Prior run state:\n{}\n\nDiff:\n```\n{}\n```\nChanged files:\n{}",
        profile.repo,
        target.mr_id.as_deref().unwrap_or("n/a"),
        target.source_branch,
        target.target_branch,
        target.ci_status.as_deref().unwrap_or("unknown"),
        target.mr_title.as_deref().unwrap_or("n/a"),
        target.mr_body.as_deref().unwrap_or("n/a"),
        target.prior_state.as_deref().unwrap_or("not found"),
        &diff_bundle.diff[..diff_bundle.diff.len().min(60_000)],
        diff_bundle.files,
    );

    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    let env_vars = if resolved_env.is_empty() {
        vec![]
    } else {
        runner::load_env_file(resolved_env)
    };
    let result = runner::run_review_backend(
        profile,
        &route.effective_backend,
        repo,
        &prompt,
        session_dir,
        route.effective_model.as_deref(),
        &env_vars,
    );
    println!("Review backend duration: {:.1}s", result.duration_secs);
    let report_path = session_dir.join("review-report.md");
    let verdict_path = session_dir.join("review-verdict.json");
    fs::write(&report_path, &result.stdout)?;
    if !result.stderr.trim().is_empty() {
        fs::write(session_dir.join("review-stderr.log"), &result.stderr)?;
    }

    match result.outcome {
        runner::ReviewProcessOutcome::Success => {
            let review_usage = usage::parse_generic_usage(&result.stdout, "review_output_log");
            let verdict = match parse_review_verdict(&result.stdout, &route, &review_usage) {
                Ok(verdict) => verdict,
                Err(err) => {
                    ledger.set_failure(
                        crate::ledger::FailureClass::BackendError,
                        crate::ledger::FailureStage::Review,
                    );
                    ledger.backend_exit_code = Some(0);
                    ledger.validation_result = Some("invalid_output".into());
                    return Err(err);
                }
            };
            fs::write(&verdict_path, serde_json::to_string_pretty(&verdict)?)?;
            println!("{}", result.stdout);
            println!("Written: {}", report_path.display());
            println!("Written: {}", verdict_path.display());
            ledger.backend_exit_code = Some(0);
            ledger.validation_result = Some(verdict.verdict.clone());
            ledger.human_required = verdict.human_required;
            ledger.confidence_impact = Some(verdict.confidence.clone());
            ledger.usage = review_usage.clone();
            let mr_body = render_review_comment(&verdict, session_dir);
            let labels = review_labels(&verdict);
            if profile.provider == "gitlab" {
                match provider::gitlab_find_mr_by_branch(profile, &target.source_branch) {
                    Ok(mr) => println!("Resolved MR: {}", mr.url),
                    Err(err) => {
                        eprintln!("warning: failed to resolve GitLab MR for branch: {:#}", err)
                    }
                }
            }
            if let Err(err) =
                provider::post_review_comment(profile, &target.source_branch, &mr_body, &labels)
            {
                eprintln!("warning: failed to post MR review comment: {:#}", err);
            }
            if verdict.human_required {
                println!("Review requires human attention.");
            }
        }
        runner::ReviewProcessOutcome::ExecutableUnavailable => {
            ledger.set_failure(
                crate::ledger::FailureClass::EnvironmentError,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend is unavailable.");
            println!("Review bundle written to: {}", bundle.display());
        }
        runner::ReviewProcessOutcome::SpawnFailure => {
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::BackendLaunch,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend failed to launch.");
            println!("Review bundle written to: {}", bundle.display());
        }
        runner::ReviewProcessOutcome::NonZeroExit(code) => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(code);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend exited with status {}.", code);
            println!("Review bundle written to: {}", bundle.display());
        }
        runner::ReviewProcessOutcome::SignalTermination(signal) => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(-signal);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend terminated by signal {}.", signal);
            println!("Review bundle written to: {}", bundle.display());
        }
        runner::ReviewProcessOutcome::Timeout => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run".into());
            println!(
                "Review backend timed out after {} seconds.",
                profile.review_timeout_seconds()
            );
            println!("Review bundle written to: {}", bundle.display());
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
            let route = dry_run_route(cfg, profile, &args.mode, args);
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
            if let Some(route) = &route {
                println!("Effective:    {}", route.effective_backend);
                println!("Routing:      {}", route.routing_reason);
            }
            println!("Retries:      {}", args.retries);
            println!("Allow draft fail: {}", args.allow_draft_fail);
            println!("Prod env:         {}", args.prod);
            if !profile.validation_commands.is_empty() {
                println!("Validation:");
                for cmd in &profile.validation_commands {
                    println!("  $ {}", cmd);
                }
            }
            if !args.target.is_empty() {
                let task_type = if Path::new(&args.target)
                    .extension()
                    .is_some_and(|e| e == "json")
                {
                    "candidate JSON"
                } else {
                    "task string"
                };
                println!("Task source:  {} ({})", args.target, task_type);
            }
            println!(
                "\nSteps: fetch → worktree → {} → [validate → retry]* → commit → push → draft MR",
                route.as_ref().map(|r| r.effective_backend.as_str()).unwrap_or(args.backend.as_str())
            );
        }
        "pm" => {
            if args.target.is_empty() {
                println!("Steps: git log → test count → CI check → write pm-report.md")
            } else {
                let route = dry_run_route(cfg, profile, "pm", args);
                println!("Backend:      {}", args.backend);
                if let Some(route) = &route {
                    println!("Effective:    {}", route.effective_backend);
                    println!("Routing:      {}", route.routing_reason);
                }
                println!(
                    "Steps: collect manager memory/MRs/tickets/repo state → {} backend → structured PM plan → validated tickets in docs/tickets/",
                    route.as_ref().map(|r| r.effective_backend.as_str()).unwrap_or(args.backend.as_str())
                )
            }
        }
        "review" => {
            let route = dry_run_route(cfg, profile, "review", args);
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!("Effective:    {}", route.effective_backend);
                println!("Routing:      {}", route.routing_reason);
            }
            if let Some(mr) = args.mr.as_deref() {
                println!("Review MR:    {}", mr);
            }
            if let Some(branch) = args.branch.as_deref() {
                println!("Source branch: {}", branch);
            }
            if args.current_branch {
                println!("Source branch: current branch");
            }
            println!("Steps: fetch target/source refs → explicit diff → bundle → routed review");
        }
        "experiment" => println!(
            "Steps: worktree → {} backend (research prompt) → collect artifacts → LLM judge → commit → draft MR",
            args.backend
        ),
        other => println!("mode '{}': not yet implemented", other),
    }
    println!("\n## Safety\n- No pushes, no MRs, no provider calls (dry run)");
    Ok(())
}

/// Build the task prompt for the agent.
/// If `target` is a path to a candidates.json file, build a structured packet from the first candidate.
/// Otherwise build a mode-appropriate prompt with target as the task body.
fn build_task(profile: &Profile, wt: &Path, mode: &str, target: &str) -> String {
    // Try to load as candidate artifact
    if !target.is_empty() {
        let p = Path::new(target);
        if p.extension().is_some_and(|e| e == "json") && p.exists() {
            if let Ok(text) = fs::read_to_string(p) {
                if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                    if let Some(candidate) = artifact.candidates.first() {
                        return format_candidate_task(profile, wt, mode, candidate);
                    }
                }
            }
        }
    }

    let instruction = match mode {
        "fix" => {
            "Fix the specific issue described in the Focus section below.\n\
             Run the relevant tests to confirm the fix. All tests in the test suite must pass.\n\
             Do not push or create MRs."
        }
        "experiment" => {
            "This is a research/experiment task. Write scripts, generate Jupyter notebooks, \
             CSV exports, plots, or markdown reports directly in the working directory.\n\
             Do not worry about breaking unrelated tests. Prioritize producing observable \
             output files (*.ipynb, *.html, *.csv, *.png, *.md) over clean commits.\n\
             Do not push or create MRs."
        }
        _ => {
            "Select and implement the highest-priority improvement from the backlog, \
             recent CI failures, or test gaps.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
    };

    let mut task = format!(
        "Repository: {} ({})\nWorking directory: {}\nTarget branch: {}\n\n{}\n",
        profile.display_name,
        profile.repo,
        wt.display(),
        profile.default_target_branch,
        instruction,
    );
    if !target.is_empty() {
        task.push_str(&format!("\n## Focus\n\n{}\n", target));
    }
    task
}

fn format_candidate_task(
    profile: &Profile,
    _wt: &Path,
    mode: &str,
    c: &crate::models::Candidate,
) -> String {
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

    let closing = match mode {
        "fix" => {
            "Fix the code to satisfy every acceptance criterion above.\n\
             Run the verification steps to confirm. All tests must pass. Do not push or create MRs.\n"
        }
        "experiment" => {
            "This is a research task. Satisfy the acceptance criteria by producing output files \
             (*.ipynb, *.html, *.csv, *.png, *.md). Do not push or create MRs.\n"
        }
        _ => {
            "Implement the changes required to satisfy the acceptance criteria above.\n\
             Run tests if a test command exists and ensure they pass. Do not push or create MRs.\n"
        }
    };
    out.push_str(closing);
    out
}

#[cfg(test)]
mod tests {
    use super::preflight;
    use super::validate;
    use super::{
        apply_pm_plan, apply_route_to_ledger, build_mr_title, build_pm_plan_task,
        classify_validation_failure_progress, collect_pm_preflight, collect_ticket_summaries,
        first_markdown_heading, mark_backend_unavailable_from_output_at, parse_pm_plan,
        parse_ticket_metadata, validation_failure_no_progress_reason, RouteDecision,
        TicketMetadata, ValidationFailureProgress,
    };
    use crate::availability::{availability_for, load_state, Reason};
    use crate::config::{Profile, RoutingPolicy};
    use crate::ledger::LedgerEntry;
    use crate::models::PmPlan;
    use crate::test_support::PathGuard;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use time::OffsetDateTime;

    const CODEX_FULL_RESET: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");

    fn profile(local_path: &Path) -> Profile {
        Profile {
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: local_path.display().to_string(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: RoutingPolicy::default(),
        }
    }

    fn init_repo(repo: &Path) {
        fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo)
            .output()
            .unwrap();
        fs::write(repo.join("README.md"), "hi\n").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    fn make_fake_bin(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn apply_route_to_ledger_records_effective_model() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = LedgerEntry::new(
            "test",
            &profile(tmp.path()),
            "codex",
            "improve",
            "target",
            Some("session-1".into()),
            None,
        );
        let route = RouteDecision {
            requested_backend: "auto".into(),
            effective_backend: "codex".into(),
            requested_model: None,
            effective_model: Some("claude-sonnet-4".into()),
            routing_reason: "ticket recommendation".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
        };

        apply_route_to_ledger(&mut entry, &route);

        assert_eq!(entry.effective_model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(entry.effective_backend, "codex");
        assert_eq!(
            entry.routing_reason.as_deref(),
            Some("ticket recommendation")
        );
    }

    #[test]
    fn apply_route_to_ledger_leaves_null_when_no_model() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = LedgerEntry::new(
            "test",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );
        let route = RouteDecision {
            requested_backend: "auto".into(),
            effective_backend: "openhands".into(),
            requested_model: None,
            effective_model: None,
            routing_reason: "profile routing policy".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
        };

        apply_route_to_ledger(&mut entry, &route);

        assert_eq!(entry.effective_model, None);
        assert_eq!(entry.effective_backend, "openhands");
    }

    #[test]
    fn preflight_uses_profile_executable_override() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let claude_path = make_fake_bin(&bin_dir, "claude-explicit");
        let git_path = make_fake_bin(&bin_dir, "git");
        let _guard = PathGuard::set(git_path.parent().unwrap());

        let mut profile = profile(tmp.path());
        profile.claude_path = Some(claude_path.display().to_string());

        let result = preflight(&profile, "claude");

        assert!(result.is_ok());
    }

    #[test]
    fn ticket_summaries_include_filename_and_heading() {
        let tmp = tempfile::tempdir().unwrap();
        let tickets = tmp.path().join("docs/tickets");
        fs::create_dir_all(&tickets).unwrap();
        fs::write(tickets.join("TICKET-001-fix.md"), "# Fix login\nbody\n").unwrap();

        assert_eq!(
            collect_ticket_summaries(&tickets).unwrap(),
            vec!["- TICKET-001-fix.md: Fix login"]
        );
    }

    #[test]
    fn backend_failure_fixture_marks_unavailability() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "codex",
            Some("local/test"),
            CODEX_FULL_RESET,
            "/tmp/backend-output.log",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            parsed.kind,
            crate::quota_parser::FailureKind::QuotaExhausted
        );
        let state = load_state(&state).unwrap();
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.records[0].backend, "codex");
        assert_eq!(state.records[0].model.as_deref(), Some("local/test"));
        assert_eq!(state.records[0].reason, Reason::QuotaExhausted);
        assert!(state.records[0].unavailable_until.is_some());
    }

    #[test]
    fn unrecognized_backend_failure_does_not_invent_unavailability() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "codex",
            Some("local/test"),
            "plain old crash with no quota language",
            "/tmp/backend-output.log",
        )
        .unwrap();

        assert!(parsed.is_none());
        let decision = availability_for(
            &state,
            "codex",
            Some("local/test"),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert!(decision.eligible);
    }

    #[test]
    fn pm_preflight_requires_manager_memory() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let err = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("PM mode requires manager memory"));
    }

    #[test]
    fn pm_task_includes_preflight_context_and_rules() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "# Memory\nRemember open work.\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/tickets/TICKET-002-auth.md"),
            "# Fix push auth\n",
        )
        .unwrap();

        let ctx = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap();
        let task = build_pm_plan_task(&profile(tmp.path()), &ctx, "Fix push auth").unwrap();
        assert!(task.contains("## Preflight Context"));
        assert!(task.contains("Remember open work."));
        assert!(task.contains("TICKET-002-auth.md: Fix push auth"));
        assert!(task.contains("Default action is to avoid creating new tickets."));
        assert!(task.contains("Current branch: main"));
    }

    #[test]
    fn first_heading_skips_non_headings() {
        assert_eq!(
            first_markdown_heading("intro\n## Heading\n"),
            Some("Heading")
        );
    }

    #[test]
    fn parse_pm_plan_extracts_json_from_log() {
        let plan =
            parse_pm_plan("noise\n{\"title\":\"T\",\"summary\":\"S\",\"tickets\":[]}\n").unwrap();
        assert_eq!(plan.title, "T");
    }

    #[test]
    fn validation_failure_matching_baseline_is_classified_separately() {
        let progress =
            classify_validation_failure_progress(Some("same failure"), None, "same failure");
        assert_eq!(progress, ValidationFailureProgress::UnchangedFromBaseline);
        assert!(progress.unchanged_from_baseline());
        assert!(!progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_matching_previous_attempt_is_classified_separately() {
        let progress = classify_validation_failure_progress(
            Some("baseline failure"),
            Some("same failure"),
            "same failure",
        );
        assert_eq!(
            progress,
            ValidationFailureProgress::UnchangedFromPreviousAttempt
        );
        assert!(!progress.unchanged_from_baseline());
        assert!(progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_matching_both_baseline_and_previous_is_distinct() {
        let progress = classify_validation_failure_progress(
            Some("same failure"),
            Some("same failure"),
            "same failure",
        );
        assert_eq!(
            progress,
            ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt
        );
        assert!(progress.unchanged_from_baseline());
        assert!(progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_changes_are_not_misclassified() {
        let progress = classify_validation_failure_progress(
            Some("baseline failure"),
            Some("previous failure"),
            "new failure",
        );
        assert_eq!(progress, ValidationFailureProgress::Changed);
        assert!(!progress.unchanged_from_baseline());
        assert!(!progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_reasons_explain_baseline_vs_previous_attempt() {
        assert!(validation_failure_no_progress_reason(
            ValidationFailureProgress::UnchangedFromBaseline
        )
        .unwrap()
        .contains("pristine-tree baseline"));
        assert!(validation_failure_no_progress_reason(
            ValidationFailureProgress::UnchangedFromPreviousAttempt
        )
        .unwrap()
        .contains("previous attempt"));
    }

    #[test]
    fn apply_pm_plan_skips_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        let ctx = super::PmPreflight {
            rendered: String::new(),
            existing_tickets: vec!["- TICKET-001-fix.md: Fix login".into()],
            open_mrs: String::new(),
            merged_mrs: String::new(),
        };
        let plan: PmPlan = serde_json::from_str(
            r#"{"title":"Plan","summary":"Summary","tickets":[
                {"title":"Fix login","summary":"dup","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"},
                {"title":"Fix auth","summary":"new","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"}
            ]}"#,
        )
        .unwrap();

        let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
        assert_eq!(written.len(), 1);
        assert!(written[0].display().to_string().contains("fix-auth"));
    }

    #[test]
    fn parses_ticket_metadata_for_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-058-descriptive-mr-titles.md");
        fs::write(
            &ticket,
            "# TICKET-058: Descriptive MR Titles\n\nDifficulty: hard\nRisk: high\nRecommended backend: codex\nRecommended model: gpt-x\n\n## Affected Files\n- src/auth.rs\n\n## Verification Commands\n- `pytest tests/test_auth.py -x`\n",
        )
        .unwrap();
        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-058"));
        assert_eq!(meta.title.as_deref(), Some("Descriptive MR Titles"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("codex"));
        assert_eq!(meta.recommended_model.as_deref(), Some("gpt-x"));
        assert_eq!(meta.difficulty.as_deref(), Some("hard"));
        assert_eq!(meta.risk.as_deref(), Some("high"));
        assert_eq!(
            meta.verification_commands,
            vec!["pytest tests/test_auth.py -x"]
        );
    }

    #[test]
    fn mr_title_uses_ticket_context_and_preserves_draft_fail_prefix() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-058".into()),
            work_id: Some("TICKET-058".into()),
            title: Some("Descriptive MR Titles".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        assert_eq!(
            build_mr_title("fix", "real", false, Some(&ticket)),
            "[GAH] Fix: TICKET-058 Descriptive MR Titles"
        );
        assert_eq!(
            build_mr_title("fix", "real", true, Some(&ticket)),
            "[GAH][DRAFT-FAIL] Fix: TICKET-058 Descriptive MR Titles"
        );
    }

    #[test]
    fn mr_title_collision_detection_prevents_stale_id_in_title() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let tickets_dir = repo.join("docs/tickets");
        fs::create_dir_all(&tickets_dir).unwrap();

        // Write MANAGER_MEMORY.md with TICKET-074 mapped to baseline disposition
        fs::write(
            repo.join("docs/MANAGER_MEMORY.md"),
            "- [MERGED] TICKET-074: Baseline disposition classifier\n",
        )
        .unwrap();

        // Write ticket file with filename TICKET-074-fix...md but heading has a different title
        let ticket_path = tickets_dir.join("TICKET-074-fix-closed-unmerged-classification.md");
        fs::write(
            &ticket_path,
            "# TICKET-074: Fix closed unmerged MR classification\n\nGoal: Treat closed unmerged MRs as terminal\n",
        )
        .unwrap();

        // Parse should detect collision and set is_authoritative to false
        let meta = parse_ticket_metadata(&ticket_path).unwrap().unwrap();
        assert!(!meta.is_authoritative);

        // Since it's not authoritative, the MR title should NOT contain the ID "TICKET-074"
        let title = build_mr_title("fix", "real", false, Some(&meta));
        assert_eq!(title, "[GAH] Fix: Fix closed unmerged MR classification");
        assert!(!title.contains("TICKET-074"));
    }

    #[test]
    fn mr_title_missing_metadata_fallback() {
        // Without ticket metadata, it should fall back to mode + repo_id
        let title = build_mr_title("fix", "real", false, None);
        assert_eq!(title, "[GAH] Fix: real");

        let title_draft = build_mr_title("fix", "real", true, None);
        assert_eq!(title_draft, "[GAH][DRAFT-FAIL] Fix: real");
    }

    #[test]
    fn mr_title_suggested_mr_title_used() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some("Heading Title".into()),
            suggested_mr_title: Some(
                "Derive PR titles from authoritative structured work metadata".into(),
            ),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        // When suggested_mr_title is present and authoritative, use it with the ID
        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert_eq!(
            title,
            "[GAH] Fix: TICKET-093 Derive PR titles from authoritative structured work metadata"
        );
    }

    #[test]
    fn mr_title_graceful_truncation() {
        let long_title = "a".repeat(300);
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some(long_title),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert!(title.len() <= 255);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn validate_runs_shell_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        // `cd x && y` requires a shell — this was silently impossible before
        let cmds = vec!["cd sub && true".to_string()];
        assert!(validate(&cmds, tmp.path()).is_ok());
    }

    #[test]
    fn validate_reports_failing_command_output() {
        let tmp = tempfile::tempdir().unwrap();
        let cmds = vec!["echo oops >&2 && false".to_string()];
        let err = validate(&cmds, tmp.path()).unwrap_err();
        assert!(format!("{:#}", err).contains("oops"));
    }
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    worktree::git(args, cwd)
}

fn apply_diff_stats(ledger: &mut LedgerEntry, wt: &Path, target_branch: &str) {
    if let Ok(stats) = worktree::diff_stats(wt, target_branch) {
        ledger.files_changed = Some(stats.files_changed);
        ledger.insertions = Some(stats.insertions);
        ledger.deletions = Some(stats.deletions);
    }
}

fn summarize_error(err: &anyhow::Error) -> String {
    let mut text = format!("{:#}", err).replace('\n', " ");
    if text.len() > 500 {
        text.truncate(500);
        text.push_str("...");
    }
    text
}

fn dry_run_route(
    cfg: &GahConfig,
    profile: &Profile,
    mode: &str,
    args: &DispatchArgs,
) -> Option<RouteDecision> {
    let ticket_meta = if matches!(mode, "improve" | "fix") && !args.target.is_empty() {
        parse_ticket_metadata(Path::new(&args.target))
            .ok()
            .flatten()
    } else {
        None
    };
    routing::decide(
        &cfg.defaults,
        profile,
        RouteRequest {
            mode,
            requested_backend: &args.backend,
            requested_model: args.model.as_deref(),
            recommended_backend: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_backend.as_deref()),
            recommended_model: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_model.as_deref()),
            session_id: None,
            usage_summary: None,
        },
    )
    .ok()
}

fn classify_validation_failure_progress(
    baseline_failure: Option<&str>,
    previous_failure: Option<&str>,
    current_failure: &str,
) -> ValidationFailureProgress {
    let same_as_baseline = baseline_failure == Some(current_failure);
    let same_as_previous = previous_failure == Some(current_failure);
    match (same_as_baseline, same_as_previous) {
        (true, true) => ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt,
        (true, false) => ValidationFailureProgress::UnchangedFromBaseline,
        (false, true) => ValidationFailureProgress::UnchangedFromPreviousAttempt,
        (false, false) => ValidationFailureProgress::Changed,
    }
}

fn resolve_review_target(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
) -> Result<ReviewTarget> {
    if let Some(mr) = args.mr.as_deref() {
        let mr_target = provider::find_review_target_by_mr(profile, mr)?;
        return Ok(ReviewTarget {
            mr_id: Some(mr_target.id),
            mr_url: Some(mr_target.url),
            mr_title: mr_target.title,
            mr_body: mr_target.body,
            ci_status: mr_target.ci_status,
            source_branch: mr_target.source_branch.clone(),
            target_branch: fallback_target_branch(
                &profile.default_target_branch,
                Some(&mr_target.target_branch),
            ),
            prior_state: lookup_review_state_by_branch(
                cfg,
                &args.profile,
                &mr_target.source_branch,
            ),
        });
    }

    if let Some(branch) = args.branch.as_deref() {
        return review_target_from_branch(profile, branch);
    }

    if !args.target.is_empty() {
        let target_path = Path::new(&args.target);
        if let Some(ticket) = parse_ticket_metadata(target_path)? {
            if let Some(state) =
                lookup_review_state(cfg, profile, &args.profile, &args.target, &ticket)
            {
                return Ok(state);
            }
        } else {
            return review_target_from_branch(profile, &args.target);
        }
    }

    if args.current_branch {
        let repo = Path::new(&profile.local_path);
        let branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"], repo)?;
        return review_target_from_branch(profile, &branch);
    }

    anyhow::bail!(
        "review target required: pass --mr, --branch, a ticket path in --target, or --current-branch"
    )
}

fn review_target_from_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
    match provider::find_review_target_by_branch(profile, branch) {
        Ok(mr_target) => Ok(ReviewTarget {
            mr_id: Some(mr_target.id),
            mr_url: Some(mr_target.url),
            source_branch: if mr_target.source_branch.is_empty() {
                branch.to_string()
            } else {
                mr_target.source_branch
            },
            target_branch: fallback_target_branch(
                &profile.default_target_branch,
                Some(&mr_target.target_branch),
            ),
            mr_title: mr_target.title,
            mr_body: mr_target.body,
            ci_status: mr_target.ci_status,
            prior_state: None,
        }),
        Err(_) => Ok(ReviewTarget {
            mr_id: None,
            mr_url: None,
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_branch: branch.to_string(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: None,
        }),
    }
}

fn fallback_target_branch(default_branch: &str, provider_target: Option<&str>) -> String {
    provider_target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_branch)
        .to_string()
}

fn lookup_review_state(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    target: &str,
    ticket: &TicketMetadata,
) -> Option<ReviewTarget> {
    let entries = ledger::read_entries(cfg).ok()?;
    let ticket_id = ticket.ticket_id.as_deref();
    let ticket_title = ticket.title.as_deref().map(normalize_match);
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.is_some()
                && entry.error_summary.is_none()
                && (entry.target_summary.as_deref() == Some(target)
                    || ticket_id
                        .map(|id| entry.target_summary.as_deref().unwrap_or("").contains(id))
                        .unwrap_or(false)
                    || ticket_title
                        .as_ref()
                        .map(|title| {
                            normalize_match(entry.target_summary.as_deref().unwrap_or(""))
                                .contains(title)
                        })
                        .unwrap_or(false))
        })
        .map(|entry| ReviewTarget {
            mr_id: entry
                .mr_url
                .as_deref()
                .and_then(|url| url.rsplit('/').next())
                .map(str::to_string),
            mr_url: entry.mr_url.clone(),
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_branch: entry.branch.clone().unwrap_or_default(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: Some(render_prior_ledger_state(&entry)),
        })
}

fn lookup_review_state_by_branch(
    cfg: &GahConfig,
    profile_name: &str,
    branch: &str,
) -> Option<String> {
    let entries = ledger::read_entries(cfg).ok()?;
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.as_deref() == Some(branch)
        })
        .map(|entry| render_prior_ledger_state(&entry))
}

fn render_prior_ledger_state(entry: &LedgerEntry) -> String {
    format!(
        "Mode: {}\nRequested backend/model: {} / {}\nEffective backend/model: {} / {}\nValidation result: {}\nMR: {}\nSession: {}",
        entry.mode,
        entry.requested_backend,
        entry.requested_model.as_deref().unwrap_or("unknown"),
        entry.effective_backend,
        entry.effective_model.as_deref().unwrap_or("unknown"),
        entry.validation_result.as_deref().unwrap_or("unknown"),
        entry.mr_url.as_deref().unwrap_or("n/a"),
        entry.session_dir.as_deref().unwrap_or("n/a"),
    )
}

fn prepare_review_diff(
    repo: &Path,
    _profile: &Profile,
    target: &ReviewTarget,
) -> Result<ReviewDiffBundle> {
    git_output(&["fetch", "-q", "origin", "--prune"], repo)?;
    git_output(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.target_branch, target.target_branch
            ),
        ],
        repo,
    )?;
    git_output(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.source_branch, target.source_branch
            ),
        ],
        repo,
    )?;

    let target_ref = format!("origin/{}", target.target_branch);
    let source_ref = format!("origin/{}", target.source_branch);
    let diff = git_output(&["diff", &format!("{target_ref}...{source_ref}")], repo)?;
    let files = git_output(
        &[
            "diff",
            "--name-only",
            &format!("{target_ref}...{source_ref}"),
        ],
        repo,
    )?;
    if diff.trim().is_empty() {
        anyhow::bail!(empty_review_diff_diagnostics(
            repo,
            target,
            &target_ref,
            &source_ref
        ));
    }
    Ok(ReviewDiffBundle { diff, files })
}

fn empty_review_diff_diagnostics(
    repo: &Path,
    target: &ReviewTarget,
    target_ref: &str,
    source_ref: &str,
) -> String {
    let current_branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(error: {e:#})"));
    let target_sha =
        git_output(&["rev-parse", target_ref], repo).unwrap_or_else(|e| format!("(error: {e:#})"));
    let source_sha =
        git_output(&["rev-parse", source_ref], repo).unwrap_or_else(|e| format!("(error: {e:#})"));
    let diff_stat = git_output(
        &["diff", "--stat", &format!("{target_ref}...{source_ref}")],
        repo,
    )
    .unwrap_or_else(|e| format!("(error: {e:#})"));
    format!(
        "empty review diff\nprofile.local_path: {}\ncurrent branch: {}\nsource branch: {}\ntarget branch: {}\nfetched refs: {}, {}\ngit rev-parse target: {}\ngit rev-parse source: {}\ngit diff --stat:\n{}\nsuggestion: fetch the source branch or pass --branch/--mr for the open review target explicitly",
        repo.display(),
        current_branch,
        target.source_branch,
        target.target_branch,
        source_ref,
        target_ref,
        target_sha,
        source_sha,
        diff_stat,
    )
}

#[derive(Debug, Clone)]
struct PmPreflight {
    rendered: String,
    existing_tickets: Vec<String>,
    open_mrs: String,
    merged_mrs: String,
}

#[derive(Debug, Clone)]
struct ReviewTarget {
    mr_id: Option<String>,
    mr_url: Option<String>,
    mr_title: Option<String>,
    mr_body: Option<String>,
    ci_status: Option<String>,
    source_branch: String,
    target_branch: String,
    prior_state: Option<String>,
}

#[derive(Debug, Clone)]
struct ReviewDiffBundle {
    diff: String,
    files: String,
}

fn parse_pm_plan(log_text: &str) -> Result<PmPlan> {
    let json = extract_first_json_object(log_text)
        .ok_or_else(|| anyhow::anyhow!("PM planner did not return valid JSON"))?;
    let plan = serde_json::from_str::<PmPlan>(&json)?;
    if plan.title.trim().is_empty() || plan.summary.trim().is_empty() {
        anyhow::bail!("PM plan missing title or summary");
    }
    Ok(plan)
}

fn extract_first_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        for end in start..bytes.len() {
            let ch = bytes[end] as char;
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &text[start..=end];
                        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                            return Some(candidate.to_string());
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn apply_pm_plan(repo: &Path, ctx: &PmPreflight, plan: &PmPlan) -> Result<Vec<PathBuf>> {
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir)?;
    let next_id = next_ticket_id(&tickets_dir)?;
    let mut written = vec![];
    let mut id = next_id;
    for ticket in &plan.tickets {
        if should_skip_ticket(ctx, ticket) {
            continue;
        }
        validate_ticket(ticket)?;
        let slug = slugify(&ticket.title);
        let filename = format!("TICKET-{:03}-{}.md", id, slug);
        let path = tickets_dir.join(filename);
        fs::write(&path, render_ticket(ticket, id))?;
        written.push(path);
        id += 1;
    }
    Ok(written)
}

fn should_skip_ticket(ctx: &PmPreflight, ticket: &PmPlanTicket) -> bool {
    let title = normalize_match(&ticket.title);
    if title.is_empty() {
        return true;
    }
    ctx.existing_tickets
        .iter()
        .any(|item| normalize_match(item).contains(&title))
        || normalize_match(&ctx.open_mrs).contains(&title)
        || normalize_match(&ctx.merged_mrs).contains(&title)
}

fn validate_ticket(ticket: &PmPlanTicket) -> Result<()> {
    if ticket.title.trim().is_empty() || ticket.summary.trim().is_empty() {
        anyhow::bail!("ticket missing title or summary");
    }
    if !matches!(ticket.difficulty.as_str(), "easy" | "medium" | "hard") {
        anyhow::bail!("ticket '{}' has invalid difficulty", ticket.title);
    }
    if !matches!(ticket.risk.as_str(), "low" | "medium" | "high") {
        anyhow::bail!("ticket '{}' has invalid risk", ticket.title);
    }
    if ticket.acceptance_criteria.is_empty() || ticket.verification_commands.is_empty() {
        anyhow::bail!(
            "ticket '{}' missing acceptance or verification",
            ticket.title
        );
    }
    Ok(())
}

fn render_ticket(ticket: &PmPlanTicket, id: usize) -> String {
    let mut out = format!(
        "# TICKET-{id:03}: {title}\n\n\
Goal: {summary}\n\n\
Difficulty: {difficulty}\n\
Risk: {risk}\n\
Recommended backend: {backend}\n\n\
## Why This Is Uncovered\n{reason}\n\n\
## Affected Files\n",
        id = id,
        title = ticket.title,
        summary = ticket.summary,
        difficulty = ticket.difficulty,
        risk = ticket.risk,
        backend = ticket
            .recommended_backend
            .as_deref()
            .unwrap_or("unspecified"),
        reason = ticket.uncovered_reason,
    );
    for file in &ticket.affected_files {
        out.push_str(&format!("- {}\n", file));
    }
    if !ticket.duplicate_evidence.is_empty() {
        out.push_str("\n## Duplicate Evidence Considered\n");
        for item in &ticket.duplicate_evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out.push_str("\n## Acceptance Criteria\n");
    for item in &ticket.acceptance_criteria {
        out.push_str(&format!("- {}\n", item));
    }
    out.push_str("\n## Verification Commands\n");
    for cmd in &ticket.verification_commands {
        out.push_str(&format!("- `{}`\n", cmd));
    }
    out
}

fn next_ticket_id(tickets_dir: &Path) -> Result<usize> {
    let mut max_id = 0usize;
    if tickets_dir.exists() {
        for entry in fs::read_dir(tickets_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("TICKET-") {
                if let Some((num, _)) = rest.split_once('-') {
                    max_id = max_id.max(num.parse::<usize>().unwrap_or(0));
                }
            }
        }
    }
    Ok(max_id + 1)
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize_match(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_route_to_ledger(ledger: &mut LedgerEntry, route: &RouteDecision) {
    ledger.backend = route.effective_backend.clone();
    ledger.requested_backend = route.requested_backend.clone();
    ledger.effective_backend = route.effective_backend.clone();
    ledger.requested_model = route.requested_model.clone();
    ledger.effective_model = route.effective_model.clone();
    ledger.routing_reason = Some(route.routing_reason.clone());
    ledger.fallback_used = route.fallback_used;
    ledger.confidence_impact = route.confidence_impact.clone();
    ledger.human_required = route.human_required;
}

fn decide_route(
    cfg: &GahConfig,
    profile: &Profile,
    req: RouteRequest<'_>,
    ledger: &mut LedgerEntry,
) -> Result<RouteDecision> {
    match routing::decide(&cfg.defaults, profile, req) {
        Ok(route) => Ok(route),
        Err(err) => {
            if err.downcast_ref::<RouteError>().is_some() {
                ledger.set_failure(
                    crate::ledger::FailureClass::HumanBlocked,
                    crate::ledger::FailureStage::Route,
                );
            } else if format!("{:#}", err).contains("parsing availability state") {
                ledger.set_failure(
                    crate::ledger::FailureClass::EnvironmentError,
                    crate::ledger::FailureStage::Route,
                );
            }
            Err(err)
        }
    }
}

fn route_identity(backend: &str, model: Option<&str>) -> String {
    format!("{backend}\u{0}{}", model.unwrap_or(""))
}

fn mark_backend_unavailable_from_output(
    backend: &str,
    model: Option<&str>,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    mark_backend_unavailable_from_output_at(
        &crate::availability::resolve_state_path(),
        backend,
        model,
        log_text,
        log_path,
    )
}

fn mark_backend_unavailable_from_output_at(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    let now = OffsetDateTime::now_utc();
    let Some(parsed) = crate::quota_parser::parse(backend, log_text, now) else {
        return Ok(None);
    };

    let unavailable_until = if let Some(reset_at) = parsed.reset_at.as_deref() {
        OffsetDateTime::parse(reset_at, &Rfc3339).ok()
    } else {
        parsed
            .retry_after_seconds
            .map(|secs| now + time::Duration::seconds(secs as i64))
    };
    let reason = match parsed.kind {
        crate::quota_parser::FailureKind::QuotaExhausted => {
            crate::availability::Reason::QuotaExhausted
        }
        crate::quota_parser::FailureKind::RateLimited => crate::availability::Reason::RateLimited,
    };
    let summary = format!(
        "{}; confidence={:?}; log={}",
        parsed.matched_evidence, parsed.confidence, log_path
    );
    crate::availability::record_unavailable(
        state_path,
        backend,
        model.filter(|m| !m.is_empty()),
        reason,
        crate::availability::Source::BackendError,
        unavailable_until,
        Some(summary),
        now,
    )?;
    Ok(Some(parsed))
}

#[derive(Debug, Clone, Default)]
struct TicketMetadata {
    ticket_id: Option<String>,
    work_id: Option<String>,
    title: Option<String>,
    suggested_mr_title: Option<String>,
    difficulty: Option<String>,
    risk: Option<String>,
    recommended_backend: Option<String>,
    recommended_model: Option<String>,
    verification_commands: Vec<String>,
    affected_files: Vec<String>,
    is_authoritative: bool,
}

fn parse_ticket_metadata(path: &Path) -> Result<Option<TicketMetadata>> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path)?;
    let ticket_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| {
            let mut parts = stem.split('-');
            match (parts.next(), parts.next()) {
                (Some("TICKET"), Some(number)) if !number.is_empty() => {
                    Some(format!("TICKET-{number}"))
                }
                _ => None,
            }
        });
    let raw_heading = first_markdown_heading(&body);
    let mut work_id_from_heading = None;
    if let Some(heading) = raw_heading {
        let trimmed = heading.trim();
        if trimmed.starts_with("TICKET-") {
            if let Some((id, _)) = trimmed.split_once(':') {
                work_id_from_heading = Some(id.trim().to_string());
            }
        }
    }
    let title = raw_heading.map(|title| normalize_ticket_title(title.into()));
    let mut meta = TicketMetadata {
        ticket_id,
        title,
        ..TicketMetadata::default()
    };
    for line in body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Risk:") {
            meta.risk = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Recommended backend:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_backend = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Recommended model:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_model = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Goal:") {
            let value = value.trim();
            if meta.title.is_none() && !value.is_empty() {
                meta.title = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Suggested MR Title:") {
            meta.suggested_mr_title = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Work ID:") {
            meta.work_id = Some(value.trim().to_string());
        } else if line.starts_with("- `") && line.ends_with('`') {
            meta.verification_commands.push(
                line.trim_start_matches("- `")
                    .trim_end_matches('`')
                    .to_string(),
            );
        } else if let Some(value) = line.strip_prefix("- ") {
            if value.contains('/') || value.contains('.') {
                meta.affected_files.push(value.to_string());
            }
        }
    }
    if meta.work_id.is_none() {
        meta.work_id = work_id_from_heading;
    }

    let mut is_authoritative = false;
    if let Some(ref file_id) = meta.ticket_id {
        if let Some(ref cont_id) = meta.work_id {
            if file_id == cont_id {
                is_authoritative = true;
            }
        }
    }
    if is_authoritative {
        let repo_dir = path.parent().and_then(|p| p.parent());
        let manager_memory_path = repo_dir.map(|p| p.join("MANAGER_MEMORY.md"));
        if let Some(ref p) = manager_memory_path {
            if p.exists() {
                if let Ok(content) = fs::read_to_string(p) {
                    let file_id = meta.ticket_id.as_ref().unwrap();
                    for line in content.lines() {
                        if line.contains(file_id) {
                            if let Some(ref title) = meta.title {
                                let norm_line = normalize_match(line);
                                let norm_title = normalize_match(title);
                                if !norm_line.contains(&norm_title) {
                                    is_authoritative = false;
                                    break;
                                }
                            } else {
                                is_authoritative = false;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    meta.is_authoritative = is_authoritative;

    Ok(Some(meta))
}

fn normalize_ticket_title(title: String) -> String {
    title
        .split_once(':')
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(title)
}

fn render_ticket_label(ticket: Option<&TicketMetadata>) -> String {
    let Some(ticket) = ticket else {
        return "n/a".into();
    };
    match (ticket.ticket_id.as_deref(), ticket.title.as_deref()) {
        (Some(ticket_id), Some(title)) => format!("{ticket_id} {title}"),
        (Some(ticket_id), None) => ticket_id.to_string(),
        (None, Some(title)) => title.to_string(),
        (None, None) => "n/a".into(),
    }
}

fn truncate_title(title: &str, limit: usize) -> String {
    if title.len() <= limit {
        title.to_string()
    } else {
        let mut truncated = String::new();
        let mut char_count = 0;
        for c in title.chars() {
            if char_count < limit - 3 {
                truncated.push(c);
                char_count += 1;
            } else {
                break;
            }
        }
        truncated.push_str("...");
        truncated
    }
}

fn build_mr_title(
    mode: &str,
    repo_id: &str,
    validation_failed: bool,
    ticket: Option<&TicketMetadata>,
) -> String {
    let mode_label = match mode {
        "fix" => "Fix",
        "improve" => "Improve",
        "review" => "Review",
        "pm" => "Plan",
        other => other,
    };
    let prefix = if validation_failed {
        "[GAH][DRAFT-FAIL]"
    } else {
        "[GAH]"
    };
    let title_string = if let Some(ticket) = ticket {
        let title_text = ticket
            .suggested_mr_title
            .as_deref()
            .or(ticket.title.as_deref());
        if let Some(title) = title_text {
            let detail = if ticket.is_authoritative {
                if let Some(ref work_id) = ticket.work_id {
                    format!("{work_id} {title}")
                } else if let Some(ref ticket_id) = ticket.ticket_id {
                    format!("{ticket_id} {title}")
                } else {
                    title.to_string()
                }
            } else {
                title.to_string()
            };
            format!("{prefix} {mode_label}: {detail}")
        } else {
            format!("{prefix} {mode_label}: {repo_id}")
        }
    } else {
        format!("{prefix} {mode_label}: {repo_id}")
    };
    truncate_title(&title_string, 255)
}

fn parse_review_verdict(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
) -> Result<crate::models::ReviewVerdict> {
    let json = extract_first_json_object(review_text)
        .ok_or_else(|| anyhow::anyhow!("reviewer did not return verdict JSON"))?;
    let mut verdict = serde_json::from_str::<crate::models::ReviewVerdict>(&json)?;
    if route.fallback_used && verdict.verdict == "APPROVE_STRONG" {
        verdict.verdict = "APPROVE_WEAK".into();
    }
    if route.fallback_used {
        verdict.human_required = true;
        if verdict.confidence == "high" {
            verdict.confidence = "medium".into();
        }
    }
    if matches!(verdict.verdict.as_str(), "APPROVE_WEAK" | "HUMAN_REVIEW") {
        verdict.human_required = true;
    }
    verdict.reviewer_backend = Some(route.effective_backend.clone());
    verdict.reviewer_model = route.effective_model.clone();
    verdict.requested_backend = Some(route.requested_backend.clone());
    verdict.effective_backend = Some(route.effective_backend.clone());
    verdict.requested_model = route.requested_model.clone();
    verdict.effective_model = route.effective_model.clone();
    verdict.fallback_used = Some(route.fallback_used);
    verdict.usage_source = parsed_usage.usage_source.clone();
    verdict.input_tokens = parsed_usage.input_tokens;
    verdict.output_tokens = parsed_usage.output_tokens;
    verdict.total_tokens = parsed_usage.total_tokens;
    verdict.estimated_cost_usd = parsed_usage.estimated_cost_usd;
    verdict.actual_cost_usd = parsed_usage.actual_cost_usd;
    Ok(verdict)
}

fn render_review_comment(verdict: &crate::models::ReviewVerdict, session_dir: &Path) -> String {
    let mut out = format!(
        "GAH review verdict: `{}`\n\nConfidence: `{}`\nHuman required: `{}`\nReviewer: `{}` / `{}`\nArtifacts: `{}`\n",
        verdict.verdict,
        verdict.confidence,
        verdict.human_required,
        verdict.effective_backend.as_deref().unwrap_or("unknown"),
        verdict.effective_model.as_deref().unwrap_or("unknown"),
        session_dir.display(),
    );
    if !verdict.blocking_findings.is_empty() {
        out.push_str("\nBlocking findings:\n");
        for item in &verdict.blocking_findings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out
}

fn review_labels(verdict: &crate::models::ReviewVerdict) -> Vec<&'static str> {
    match verdict.verdict.as_str() {
        "APPROVE_STRONG" => vec!["gah-ready-for-human"],
        "APPROVE_WEAK" => vec!["gah-review-weak", "gah-human-review"],
        "NEEDS_FIX" | "REJECT" => vec!["gah-needs-fix"],
        "HUMAN_REVIEW" => vec!["gah-human-review"],
        _ => vec![],
    }
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
