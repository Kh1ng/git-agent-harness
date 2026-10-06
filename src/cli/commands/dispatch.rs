// Command execution for `gah dispatch` (ticket #408).

use anyhow::Result;
use uuid::Uuid;

use crate::dispatch::DispatchArgs as CliDispatchArgs;
use crate::{config, controller as controller_runtime, runner};

pub struct Args {
    pub profile: String,
    pub mode: String,
    pub backend: String,
    pub target: String,
    pub branch: Option<String>,
    pub mr: Option<String>,
    pub current_branch: bool,
    pub budget: u32,
    pub dry_run: bool,
    pub config_path: Option<String>,
    pub oh_profile: Option<String>,
    pub model: Option<String>,
    pub retries: u32,
    pub allow_draft_fail: bool,
    pub prod: bool,
    pub issue_intake_override: bool,
    pub allow_unknown_red_baseline: bool,
    pub escalate: bool,
    pub existing_branch: Option<String>,
    pub skip_validation_gate: bool,
    pub manual_worker: bool,
    pub reasoning_effort: Option<String>,
}

impl From<Args> for CliDispatchArgs {
    fn from(args: Args) -> Self {
        CliDispatchArgs {
            profile: args.profile,
            mode: args.mode,
            backend: args.backend,
            target: args.target,
            branch: args.branch,
            mr: args.mr,
            current_branch: args.current_branch,
            dry_run: args.dry_run,
            oh_profile: args.oh_profile,
            model: args.model,
            retries: args.retries,
            allow_draft_fail: args.allow_draft_fail,
            prod: args.prod,
            issue_intake_override: args.issue_intake_override,
            allow_unknown_red_baseline: args.allow_unknown_red_baseline,
            escalate: args.escalate,
            existing_branch: args.existing_branch,
            expected_review_generation: None,
            skip_validation_gate: args.skip_validation_gate,
            dispatch_reason: None,
            prior_attempt_context: None,
            work_id: None,
            run_id: None,
            route_admission: None,
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    runner::install_shutdown_handler()?;
    let mut cfg = config::load(args.config_path.as_deref())?;
    if args.manual_worker {
        prepare_manual_worker(&mut cfg, &args)?;
        println!(
            "Starting manual worker: {}/{} for {}",
            args.backend,
            args.model.as_deref().unwrap_or(""),
            args.target
        );
    }
    let run_id = Uuid::new_v4().to_string();
    let resolved_config_path = config::resolve_config_path(args.config_path.as_deref());
    // The loop keeps its own profile lock. Explicit extra jobs coordinate via
    // the same atomic per-work claims as loop workers, never reconcile or stop it.
    let _lock = if args.manual_worker {
        None
    } else {
        Some(controller_runtime::acquire_profile_lock(
            &args.profile,
            &resolved_config_path,
        )?)
    };
    if !args.manual_worker {
        let mut ledger_entries = crate::ledger::read_entries(&cfg)?;
        controller_runtime::reconcile_abandoned_dispatches(
            &cfg,
            &args.profile,
            &mut ledger_entries,
        )?;
    }
    let manual_worker = args.manual_worker;
    let dispatch_reason = manual_worker.then(|| {
        "Operator started an extra worker outside automatic capacity admission".to_string()
    });
    let dispatch_args = CliDispatchArgs {
        run_id: Some(run_id),
        dispatch_reason,
        ..args.into()
    };
    let outcome =
        controller_runtime::run_dispatch_and_record(&cfg, "dispatch", None, &dispatch_args)?;
    if manual_worker {
        if let Some(reason) = outcome {
            anyhow::bail!("Manual worker did not start: {reason}");
        }
    }
    Ok(())
}

fn prepare_manual_worker(cfg: &mut config::GahConfig, args: &Args) -> Result<()> {
    let model = args
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("manual worker requires an explicit model"))?;
    if args.backend == "auto" || args.backend.trim().is_empty() || args.target.trim().is_empty() {
        anyhow::bail!("manual worker requires an explicit backend and job target");
    }
    let profile = config::get_profile_mut(cfg, &args.profile)?;
    if let Some(effort) = &args.reasoning_effort {
        profile.set_agent_effort(&args.backend, effort)?;
    }
    // Dispatch already has no loop node-admission channel. Removing only this
    // run's model cap also prevents waiting silently on the automatic pool cap.
    profile
        .max_concurrent_per_model
        .remove(&format!("{}/{model}", args.backend));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn manual_launch_overrides_only_its_model_cap_and_reasoning_in_memory() {
        let mut cfg: crate::config::GahConfig = toml::from_str("[profiles]\n").unwrap();
        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile
            .max_concurrent_per_model
            .insert("codex/gpt-6.1-sol".into(), 1);
        profile
            .max_concurrent_per_model
            .insert("claude/opus".into(), 2);
        profile.codex_args = vec!["--sandbox".into(), "workspace-write".into()];
        cfg.profiles.insert("repo".into(), profile);
        let args = super::Args {
            profile: "repo".into(),
            mode: "improve".into(),
            backend: "codex".into(),
            target: "ticket.md".into(),
            model: Some("gpt-6.1-sol".into()),
            manual_worker: true,
            reasoning_effort: Some("high".into()),
            branch: None,
            mr: None,
            current_branch: false,
            budget: 1,
            dry_run: true,
            config_path: None,
            oh_profile: None,
            retries: 0,
            allow_draft_fail: false,
            prod: false,
            issue_intake_override: false,
            allow_unknown_red_baseline: false,
            escalate: false,
            existing_branch: None,
            skip_validation_gate: false,
        };
        super::prepare_manual_worker(&mut cfg, &args).unwrap();
        let profile = &cfg.profiles["repo"];
        assert!(!profile
            .max_concurrent_per_model
            .contains_key("codex/gpt-6.1-sol"));
        assert_eq!(profile.max_concurrent_per_model["claude/opus"], 2);
        assert_eq!(profile.agent_efforts()["codex"], "high");
        assert_eq!(profile.codex_args[0], "--sandbox");
    }
}
