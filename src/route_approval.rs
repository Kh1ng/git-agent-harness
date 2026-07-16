use crate::{config, ledger, routing, RouteApprovalCommands};
use anyhow::{bail, Result};

pub(crate) fn run(command: RouteApprovalCommands) -> Result<()> {
    let (profile, work_id, backend, model, config_path, granted, dry_run) = match command {
        RouteApprovalCommands::Grant {
            profile,
            work_id,
            backend,
            model,
            dry_run,
            config_path,
        } => (profile, work_id, backend, model, config_path, true, dry_run),
        RouteApprovalCommands::Revoke {
            profile,
            work_id,
            backend,
            model,
            dry_run,
            config_path,
        } => (
            profile,
            work_id,
            backend,
            model,
            config_path,
            false,
            dry_run,
        ),
    };

    let cfg = config::load(config_path.as_deref())?;
    let prof = config::get_profile(&cfg, &profile)?;
    let model = model.as_deref();

    if granted && !routing::policy::candidate_requires_approval(&prof.routing, &backend, model) {
        bail!(
            "route approval for {}/{} on profile '{}' is not configured to require approval",
            backend,
            model.unwrap_or("default"),
            profile
        );
    }

    let action = if granted { "granted" } else { "revoked" };
    let model_for_log = model.unwrap_or("default");

    if dry_run {
        println!(
            "DRY RUN: would {action} paid route approval for work_id '{work_id}' on {backend}/{model_for_log}"
        );
        return Ok(());
    }

    let entry = ledger::LedgerEntry::new_paid_route_approval(
        &profile, prof, &work_id, &backend, model, granted,
    );
    let path = ledger::append(&cfg, &entry)?;
    println!(
        "Paid route approval {action} for work_id '{work_id}' on {backend}/{model_for_log} ({})",
        path.display()
    );
    Ok(())
}
