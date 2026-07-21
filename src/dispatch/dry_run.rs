use super::attempts::routing_runtime_state;
use super::identity::timestamp;
use super::issues::parse_ticket_metadata;
use super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::routing::{self, RouteDecision, RouteRequest, TaskRoutingContext};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(in crate::dispatch) fn dry_run(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
) -> Result<()> {
    println!("DRY RUN — no mutations will be performed\n");
    println!("## What would happen\n");
    let ts = timestamp();
    let branch = format!("gah/{}-{}", profile.repo_id, ts);
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
            } else if route.is_none() {
                let cloud = args.backend == "cloud-coder";
                let default_model = cfg.defaults.llm_model(cloud);
                let model_name = args.model.as_deref().unwrap_or(&default_model);
                println!("LLM model:    {}", model_name);
                println!("LLM base:     {}", cfg.defaults.llm_base_url());
            }
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!(
                    "Effective:    {}/{}",
                    route.effective_backend,
                    route.effective_model.as_deref().unwrap_or("default")
                );
                println!("Routing:      {}", route.routing_reason);
                if let Some(summary) = route
                    .routing_diagnostics
                    .as_ref()
                    .and_then(|diagnostics| diagnostics.human_summary.as_deref())
                {
                    println!("Route detail: {}", summary);
                }
            }
            println!("Retries:      {}", args.retries);
            println!("Allow draft fail: {}", args.allow_draft_fail);
            println!("Issue intake override: {}", args.issue_intake_override);
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
                    println!(
                        "Effective:    {}/{}",
                        route.effective_backend,
                        route.effective_model.as_deref().unwrap_or("default")
                    );
                    println!("Routing:      {}", route.routing_reason);
                }
                println!(
                    "Steps: collect guidance/issues/PRs/repo state → {} backend → validated pm-plan-v1.json (no provider writes)",
                    route.as_ref().map(|r| r.effective_backend.as_str()).unwrap_or(args.backend.as_str())
                )
            }
        }
        "review" => {
            let route = dry_run_route(cfg, profile, "review", args);
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!(
                    "Effective:    {}/{}",
                    route.effective_backend,
                    route.effective_model.as_deref().unwrap_or("default")
                );
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
    let mut dry_ledger = LedgerEntry::new(
        &args.profile,
        profile,
        &args.backend,
        mode,
        &args.target,
        None,
        None,
    );
    dry_ledger.work_id = ticket_meta
        .as_ref()
        .and_then(|meta| meta.work_id.clone().or_else(|| meta.ticket_id.clone()));
    let runtime = routing_runtime_state(cfg, &dry_ledger).unwrap_or_default();
    routing::decide_for_task_with_state(
        &cfg.defaults,
        profile,
        RouteRequest {
            last_failure_class: None,
            mode,
            requested_backend: config::canonical_backend_name(&args.backend),
            requested_model: args.model.as_deref(),
            recommended_backend: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_backend.as_deref()),
            recommended_model: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_model.as_deref()),
            session_id: None,
            usage_summary: None,
            exact_route_required: false,
        },
        TaskRoutingContext {
            task_class: ticket_meta
                .as_ref()
                .and_then(|meta| meta.task_class.as_deref()),
            difficulty: ticket_meta
                .as_ref()
                .and_then(|meta| meta.difficulty.as_deref()),
            risk: ticket_meta.as_ref().and_then(|meta| meta.risk.as_deref()),
        },
        &runtime,
    )
    .ok()
}
