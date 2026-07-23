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
            work_id: None,
            run_id: None,
            route_ready: None,
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    runner::install_shutdown_handler()?;
    let cfg = config::load(args.config_path.as_deref())?;
    let run_id = Uuid::new_v4().to_string();
    let resolved_config_path = config::resolve_config_path(args.config_path.as_deref());
    let _lock = controller_runtime::acquire_profile_lock(&args.profile, &resolved_config_path)?;
    let dispatch_args = CliDispatchArgs {
        run_id: Some(run_id),
        ..args.into()
    };
    let _ = controller_runtime::run_dispatch_and_record(&cfg, "dispatch", None, &dispatch_args)?;
    Ok(())
}
