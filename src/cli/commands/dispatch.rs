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
    pub enforce_job_file: bool,
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
    // Override inherited opt-in: only this CLI flag authorizes job commands.
    std::env::set_var(
        "GAH_ENFORCE_JOB_FILE",
        if args.enforce_job_file { "1" } else { "0" },
    );
    if args.enforce_job_file && !matches!(args.mode.as_str(), "fix" | "improve") {
        anyhow::bail!("--enforce-job-file requires --mode fix or --mode improve");
    }
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
    // The loop keeps its own profile lock. An explicit extra job never waits
    // for it, reconciles, or stops the loop; it takes the loop's per-work
    // claim instead (see `ManualClaim`).
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
    let dispatch_reason = manual_worker
        .then(|| "Operator started an extra worker outside the per-model job limit".to_string());
    let mut dispatch_args = CliDispatchArgs {
        run_id: Some(run_id),
        dispatch_reason,
        ..args.into()
    };
    let claim = if manual_worker && !dispatch_args.dry_run {
        let claim = ManualClaim::take(&cfg, &dispatch_args)?;
        // The same node admission a loop worker passes: free memory above the
        // floor with this job's reservation, CPU and pressure. Only the
        // per-model job limit and the loop's worker count are bypassed.
        dispatch_args.route_admission =
            Some(controller_runtime::RouteNodeAdmission::single_worker(
                controller_runtime::NextAction::DispatchTicket {
                    ticket_path: dispatch_args.target.clone(),
                    work_id: Some(claim.work_id.clone()),
                    recommended_backend: Some(dispatch_args.backend.clone()),
                    recommended_model: dispatch_args.model.clone(),
                    reason: "manual worker".to_string(),
                },
                cfg.defaults.node_capacity,
            ));
        Some(claim)
    } else {
        None
    };
    // The start event names the claimed work id, which is how the loop's
    // reconciliation tells this live run from an abandoned one.
    let claimed_work_id = claim.as_ref().map(|claim| claim.work_id.as_str());
    let outcome = controller_runtime::run_dispatch_and_record(
        &cfg,
        "dispatch",
        claimed_work_id,
        &dispatch_args,
    )?;
    if manual_worker {
        if let Some(reason) = outcome {
            anyhow::bail!("Manual worker did not start: {reason}");
        }
    }
    Ok(())
}

/// The loop's atomic per-work claim, held by a
/// manual worker for its whole run. The loop takes the same claim before it
/// starts a job, so neither can start work the other holds, and
/// `work_claim::record_route` applies to manual runs too.
struct ManualClaim {
    scope: String,
    work_id: String,
}

impl ManualClaim {
    /// Refuses a target that resolves to no work id: it could not be
    /// claimed, so the loop could start the same work alongside it.
    fn take(cfg: &config::GahConfig, args: &CliDispatchArgs) -> Result<Self> {
        let profile = config::get_profile(cfg, &args.profile)?;
        let Some(work_id) = crate::dispatch::manual_worker_work_id(profile, args) else {
            anyhow::bail!(
                "Manual worker did not start: no work id could be resolved from {:?}; pass a ticket file, a candidate file or an issue number",
                args.target
            );
        };
        let work_id = crate::work_claim::normalize_work_identity(&work_id);
        let scope = crate::work_claim::canonical_claim_scope(&args.profile, &profile.repo_id);
        if !crate::work_claim::try_claim_manual_work(&scope, &work_id)? {
            anyhow::bail!(
                "Manual worker did not start: {work_id} is already being worked on by the loop or another worker"
            );
        }
        Ok(Self { scope, work_id })
    }
}

impl Drop for ManualClaim {
    fn drop(&mut self) {
        if let Err(error) = crate::work_claim::release_owned_work(&self.scope, &self.work_id) {
            eprintln!(
                "warning: could not release the claim on {}: {error:#}",
                self.work_id
            );
        }
    }
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
    // Removing only this run's model cap keeps it from waiting silently on
    // the automatic pool cap. Node admission still applies (see `run`).
    profile
        .max_concurrent_per_model
        .remove(&format!("{}/{model}", args.backend));
    Ok(())
}

#[cfg(test)]
mod tests {
    fn manual_args(target: &str) -> super::Args {
        super::Args {
            profile: "repo".into(),
            mode: "improve".into(),
            backend: "codex".into(),
            target: target.into(),
            model: Some("gpt-6.1-sol".into()),
            manual_worker: true,
            enforce_job_file: false,
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
        }
    }

    fn manual_config() -> crate::config::GahConfig {
        let mut cfg: crate::config::GahConfig = toml::from_str("[profiles]\n").unwrap();
        let profile = crate::config::tests::test_profile_for_notifications();
        cfg.profiles.insert("repo".into(), profile);
        cfg
    }

    #[test]
    fn manual_worker_claims_an_issue_number_target() {
        let tmp = tempfile::tempdir().unwrap();
        let _claims = crate::test_support::ClaimStateEnvGuard::set(tmp.path().join("claims.json"));
        let cfg = manual_config();
        for target in ["123", "#123", " #123 "] {
            let claim = super::ManualClaim::take(&cfg, &manual_args(target).into()).unwrap();
            assert_eq!(claim.work_id, "#123", "{target:?}");
            assert!(crate::work_claim::is_claimed(&claim.scope, "#123").unwrap());
            // The loop cannot take the same issue while the worker runs.
            assert!(!crate::work_claim::try_claim_work(&claim.scope, "123").unwrap());
            let scope = claim.scope.clone();
            drop(claim);
            assert!(!crate::work_claim::is_claimed(&scope, "#123").unwrap());
        }
    }

    #[test]
    fn manual_worker_refuses_a_target_with_no_work_id() {
        let tmp = tempfile::tempdir().unwrap();
        let _claims = crate::test_support::ClaimStateEnvGuard::set(tmp.path().join("claims.json"));
        let cfg = manual_config();
        let missing = tmp.path().join("missing.md").display().to_string();
        for target in ["fix the flaky test", missing.as_str()] {
            let Err(error) = super::ManualClaim::take(&cfg, &manual_args(target).into()) else {
                panic!("{target:?} ran unclaimed");
            };
            assert!(error.to_string().contains("no work id"), "{error:#}");
        }
    }

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
        let args = manual_args("ticket.md");
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
