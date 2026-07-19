use super::super::attempts::{
    apply_route_to_ledger, classify_git_operation_result, classify_worktree_result, decide_route,
    preflight, record_route_attempt, resolve_llm, run_backend,
};
use super::super::identity::timestamp;
use super::super::issues::resolve_target_to_issue_or_string;
use super::super::metrics::apply_diff_stats;
use super::super::prompts::build_task;
use super::super::publish::{
    build_experiment_mr_body, emit_human_handoff, enforce_generated_artifact_policy,
    perform_handoff_delivery, publishing_allows_publish, ExperimentMrRenderContext,
};
use super::super::text::{utf8_safe_prefix, utf8_safe_suffix};
use super::super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::RouteRequest;
use crate::{provider, runner, worktree};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn experiment(
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
            last_failure_class: None,
            mode: "experiment",
            requested_backend: config::canonical_backend_name(&args.backend),
            requested_model: args.model.as_deref(),
            recommended_backend: None,
            recommended_model: None,
            session_id: session_dir.file_name().and_then(|s| s.to_str()),
            usage_summary: None,
            exact_route_required: false,
        },
        None,
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
    let branch = format!("gah/exp-{}-{}", profile.repo_id, ts);
    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);
    let repo = Path::new(&profile.local_path);

    println!(
        "Creating worktree from {}...",
        profile.default_target_branch
    );
    let wt = classify_worktree_result(
        ledger,
        worktree::create(
            repo,
            &profile.default_target_branch,
            &branch,
            &worktree_base,
        ),
    )?;
    ledger.branch = Some(branch.clone());
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);
    let _cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;

    let issue_details =
        resolve_target_to_issue_or_string(profile, &args.target, args.issue_intake_override)?;
    if issue_details.is_some() && args.issue_intake_override {
        println!("Issue intake override enabled for explicit issue dispatch");
    }
    let task = build_task(
        profile,
        &wt,
        "experiment",
        &args.target,
        issue_details.as_ref(),
    );
    let attempt_dir = session_dir.join("attempt-1");
    fs::create_dir_all(&attempt_dir)?;

    let env_path = if !resolved_env.is_empty() {
        Some(resolved_env)
    } else {
        None
    };
    record_route_attempt(ledger, &route);
    let result = match run_backend(
        &route.effective_backend,
        profile,
        &wt,
        &task,
        &attempt_dir,
        &llm,
        route.effective_model.as_deref(),
        env_path,
        None,
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
                final_summary: None,
                agy_cli_log_delta: None,
                internal_log_delta: None,
                internal_log_path: None,
                transcript_path: None,
                agy_version: None,
            }
        }
    };
    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );
    ledger.backend_exit_code = Some(result.exit_code);
    let backend_summary = runner::output::publishable_summary(
        result.final_summary.as_deref(),
        ledger.target_summary.as_deref(),
        &wt,
    );

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
    enforce_generated_artifact_policy(profile, ledger, &wt)?;
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        if worktree::has_uncommitted_changes(&wt)? {
            ledger.commit_attempted = true;
            worktree::stage_all(&wt)?;
            worktree::ensure_staged(&wt)?;
            let commit_msg = format!("gah: experiment for {}", profile.repo_id);
            worktree::commit_msg(&wt, &commit_msg)?;
            ledger.commit_created = true;
        } else {
            ledger.commit_created = true;
        }
        let ticket_id = ledger
            .work_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        apply_diff_stats(ledger, &wt, &profile.default_target_branch);
        perform_handoff_delivery(
            cfg,
            profile,
            ledger,
            &wt,
            &branch,
            &ticket_id,
            &backend_summary,
        )?;
        worktree::cleanup(&wt, repo);
        return Ok(());
    }
    // TICKET-128: honor the per-profile publishing policy (see fix/improve
    // mode for the full rationale). Experiments may still generate code and
    // artifacts; they just must not be published as an agent-authored MR.
    if !publishing_allows_publish(profile) {
        if profile.publishing.allow_commit_message_generation {
            if worktree::has_uncommitted_changes(&wt)? {
                ledger.commit_attempted = true;
                worktree::stage_all(&wt)?;
                worktree::ensure_staged(&wt)?;
                let commit_msg = format!("gah: experiment for {}", profile.repo_id);
                worktree::commit_msg(&wt, &commit_msg)?;
                ledger.commit_created = true;
            } else {
                ledger.commit_created = true;
            }
        }
        apply_diff_stats(ledger, &wt, &profile.default_target_branch);
        emit_human_handoff(
            profile,
            ledger,
            &branch,
            "PR/MR creation or commit-message generation disabled by publishing policy",
        );
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    println!("Changes detected. Committing and pushing...");
    let mut commit_msg = format!("gah: experiment for {}", profile.repo_id);
    if !backend_summary.is_empty() {
        commit_msg.push_str("\n\n");
        commit_msg.push_str(&backend_summary);
    }
    let push_url = profile.push_url()?;
    let push_pat = profile.pat();
    if worktree::has_uncommitted_changes(&wt)? {
        ledger.commit_attempted = true;
        worktree::stage_all(&wt)?;
        worktree::ensure_staged(&wt)?;
        worktree::commit_msg(&wt, &commit_msg)?;
        ledger.commit_created = true;
    } else {
        // Backend committed its own work already (e.g. vibe) -- nothing left
        // to stage, just push what's already on HEAD.
        ledger.commit_created = true;
    }
    // Must run after the commit above -- see the fix mode call site for why.
    apply_diff_stats(ledger, &wt, &profile.default_target_branch);
    ledger.push_attempted = true;
    classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::Push,
        worktree::push_branch(&wt, &branch, &push_url, &push_pat),
    )?;
    ledger.push_succeeded = true;

    let mr_ctx = ExperimentMrRenderContext {
        backend: &route.effective_backend,
        model: &llm.model,
        artifact_count,
        answered,
        backend_summary: &backend_summary,
    };
    let mr_body = build_experiment_mr_body(&mr_ctx);
    ledger.mr_attempted = true;
    let mr = match provider::create_draft_mr(
        profile,
        &branch,
        &format!("[GAH][EXP] {}", profile.repo_id),
        &mr_body,
    ) {
        Ok(mr) => mr,
        Err(err) => {
            if profile.provider == "gitlab" {
                ledger.set_failure(
                    crate::ledger::FailureClass::EnvironmentError,
                    crate::ledger::FailureStage::MrCreate,
                );
            }
            return Err(err);
        }
    };
    ledger.mr_created = true;
    ledger.mr_url = Some(mr.url.clone());
    println!("Draft MR: {}", mr.url);
    notify_event(
        cfg,
        profile,
        NotifyEvent::MrCreated {
            url: &mr.url,
            work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
            backend: &route.effective_backend,
            model: &llm.model,
        },
    );

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
        utf8_safe_prefix(task, 500),
        utf8_safe_suffix(log, 3000),
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

#[cfg(test)]
mod tests;
