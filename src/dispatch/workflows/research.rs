//! `research`/`audit`/`estimate` job kinds (issue #848, issue #916):
//! read-only, investigation-only dispatch. No worktree, no commit, no push,
//! no PR -- runs directly against the profile's real checkout
//! (`profile.local_path`), the same way `pm` mode reads repo state without
//! creating a worktree, since none of these modes ever mutate the repo.
//!
//! `research`/`audit` have the backend self-report findings as a GitHub
//! issue via its own `gh`/`glab` CLI access (see
//! `prompts::research_instruction`/`audit_instruction`) rather than GAH
//! orchestrating issue creation in Rust -- kept deliberately simple per the
//! owner's explicit "doesn't need to be perfected, just get it in."
//!
//! `estimate` shares this same read-only scaffold (route, env, backend
//! invocation) but swaps in a narrow single-ticket prompt/parse pair
//! (`estimator::build_estimate_task`/`parse_estimate_response`) and records
//! its structured result onto the ledger's `predicted_*` fields instead of
//! `target_summary`, so predicted-vs-actual accuracy can be measured later.

use super::super::attempts::{
    apply_route_to_ledger, decide_route, preflight_identity,
    record_external_approval_consumption_for_last_attempt, record_route_attempt, resolve_llm,
    run_backend_for_identity,
};
use super::super::issues::resolve_target_to_issue_or_string;
use super::super::prompts::build_task;
use super::super::DispatchArgs;
use super::estimator::{build_estimate_task, parse_estimate_response};
use crate::config::{self, GahConfig, Profile};
use crate::job_kind::JobKind;
use crate::ledger::LedgerEntry;
use crate::routing::RouteRequest;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub(crate) fn research(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let kind = JobKind::parse(&args.mode).unwrap_or(JobKind::Research);
    let repo = Path::new(&profile.local_path);

    let route = decide_route(
        cfg,
        profile,
        RouteRequest {
            last_failure_class: None,
            mode: kind.as_str(),
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
    preflight_identity(profile, &route.identity)?;
    let llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        route.effective_model.as_deref(),
    )?;

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
    let env_path = (!resolved_env.is_empty()).then_some(resolved_env);

    let issue_details =
        resolve_target_to_issue_or_string(profile, &args.target, args.issue_intake_override)?;
    let task = if kind == JobKind::Estimate {
        let (title, objective) = match issue_details.as_ref() {
            Some(issue) => (issue.title.clone(), issue.body.clone()),
            None => (args.target.clone(), String::new()),
        };
        build_estimate_task(&title, &objective)
    } else {
        build_task(
            profile,
            repo,
            kind.as_str(),
            &args.target,
            issue_details.as_ref(),
        )
    };

    let attempt_dir = session_dir.join("attempt-1");
    fs::create_dir_all(&attempt_dir)?;
    let cargo_target = crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root)?;

    println!(
        "{} (read-only, target branch {})",
        kind, profile.default_target_branch
    );
    ledger.branch = None;
    record_route_attempt(ledger, &route)?;
    let result = run_backend_for_identity(
        cfg,
        profile_name,
        &route.identity,
        profile,
        repo,
        &task,
        &attempt_dir,
        &cargo_target,
        &llm,
        env_path,
        ledger.work_id.as_deref(),
        None,
    )?;
    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );
    ledger.backend_exit_code = Some(result.exit_code);
    record_external_approval_consumption_for_last_attempt(cfg, profile_name, profile, ledger);

    if kind == JobKind::Estimate {
        if result.exit_code != 0 {
            anyhow::bail!(
                "estimate backend exited {} ({})",
                result.exit_code,
                result.log_path
            );
        }
        let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
        let estimate = parse_estimate_response(&log_text)?;
        println!(
            "Predicted difficulty: {} (cost={} duration={})",
            estimate.difficulty,
            estimate
                .predicted_cost_usd
                .map(|c| format!("${c:.2}"))
                .unwrap_or_else(|| "unknown".to_string()),
            estimate
                .predicted_duration_seconds
                .map(|d| format!("{d:.0}s"))
                .unwrap_or_else(|| "unknown".to_string()),
        );
        ledger.predicted_difficulty = Some(estimate.difficulty);
        ledger.predicted_cost_usd = estimate.predicted_cost_usd;
        ledger.predicted_duration_seconds = estimate.predicted_duration_seconds;
        ledger.validation_result = Some("completed".into());
        return Ok(());
    }

    ledger.target_summary = result.final_summary.clone();
    ledger.validation_result = Some(if result.exit_code == 0 {
        "completed".into()
    } else {
        "backend_error".into()
    });

    // No worktree was created (repo is the real checkout, never mutated),
    // so there is nothing to clean up -- unlike experiment/improve/fix,
    // whose worktree::cleanup calls this mirrors in spirit, not code.
    Ok(())
}
