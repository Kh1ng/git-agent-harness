// Command execution for controller-facing `gah` subcommands (ticket #408).

use anyhow::Result;

use crate::cli::args::{HoldCommands, RouteApprovalCommands};
use crate::{config, controller as controller_runtime, events, ledger, runner, status, sync};

pub struct LoopArgs {
    pub profile: String,
    pub config_path: Option<String>,
    pub json: bool,
    pub once: bool,
    pub parallel: usize,
    pub skip_validation_gate: bool,
}

pub struct EventsArgs {
    pub config_path: Option<String>,
    pub profile: Option<String>,
    pub json: bool,
    pub since: String,
}

pub struct StatusArgs {
    pub profile: String,
    pub json: bool,
    pub config_path: Option<String>,
}

pub struct SyncArgs {
    pub profile: String,
    pub config_path: Option<String>,
    pub json: bool,
}

pub fn run_hold(command: HoldCommands) -> Result<()> {
    match command {
        HoldCommands::Set {
            profile,
            work_id,
            reason,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_review_hold(&profile, prof, &work_id, reason);
            let path = ledger::append(&cfg, &entry)?;
            println!(
                "Review hold set for work_id '{}' on profile '{}' ({})",
                work_id,
                profile,
                path.display()
            );
        }
        HoldCommands::Clear {
            profile,
            work_id,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_review_hold_release(&profile, prof, &work_id);
            let path = ledger::append(&cfg, &entry)?;
            println!(
                "Review hold cleared for work_id '{}' on profile '{}' ({})",
                work_id,
                profile,
                path.display()
            );
        }
    }
    Ok(())
}

pub fn run_route_approval(command: RouteApprovalCommands) -> Result<()> {
    let (profile, work_id, backend, instance, model, config_path, granted) = match command {
        RouteApprovalCommands::Grant {
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
        } => (
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
            true,
        ),
        RouteApprovalCommands::Revoke {
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
        } => (
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
            false,
        ),
    };
    let cfg = config::load(config_path.as_deref())?;
    let prof = config::get_profile(&cfg, &profile)?;
    if let Some(instance_name) = instance.as_deref() {
        let routing = prof.effective_routing(&cfg.defaults);
        let declared = routing
            .backend_instances
            .get(instance_name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown backend instance '{instance_name}' for profile '{profile}'"
                )
            })?;
        let logical_backend = declared
            .logical_backend
            .as_deref()
            .unwrap_or(declared.runner_kind.as_str());
        if logical_backend != backend {
            anyhow::bail!(
                "backend instance '{}' belongs to logical backend '{}', not '{}'",
                instance_name,
                logical_backend,
                backend
            );
        }
    }
    let entry = ledger::LedgerEntry::new_paid_route_approval_for_instance(
        &profile,
        prof,
        &work_id,
        &backend,
        instance.as_deref(),
        model.as_deref(),
        granted,
    );
    let path = ledger::append(&cfg, &entry)?;
    println!(
        "Paid route approval {} for work_id '{}' on {}{}/{} ({})",
        if granted { "granted" } else { "revoked" },
        work_id,
        backend,
        instance
            .as_deref()
            .map(|instance| format!(" [{instance}]"))
            .unwrap_or_default(),
        model.as_deref().unwrap_or("default"),
        path.display()
    );
    Ok(())
}

pub fn run_loop(args: LoopArgs) -> Result<()> {
    runner::install_shutdown_handler()?;
    let cfg = config::load(args.config_path.as_deref())?;
    let resolved_config_path = config::resolve_config_path(args.config_path.as_deref());
    let parallel = controller_runtime::loop_parallel_argument(
        args.once,
        args.parallel,
        config::get_profile(&cfg, &args.profile)?.max_parallel_workers() as usize,
    );
    if args.once {
        // `--once` still does real execution (spawns backends, claims tickets,
        // writes ledger entries) so it must coordinate via the same profile
        // lock as the daemon (`gah loop` with no `--once`).
        let _lock = controller_runtime::acquire_profile_lock(&args.profile, &resolved_config_path)?;
        controller_runtime::run_once(
            &cfg,
            &args.profile,
            args.json,
            parallel,
            args.skip_validation_gate,
        )?;
    } else {
        controller_runtime::run_loop(
            &cfg,
            &args.profile,
            args.json,
            parallel,
            args.skip_validation_gate,
            &resolved_config_path,
        )?;
    }
    Ok(())
}

pub fn run_events(args: EventsArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    events::run(&cfg, &args.since, args.profile.as_deref(), args.json)?;
    Ok(())
}

pub fn run_status(args: StatusArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    status::run(&cfg, &args.profile, args.json)?;
    Ok(())
}

pub fn run_sync(args: SyncArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    sync::run(&cfg, &args.profile, args.json)?;
    Ok(())
}
