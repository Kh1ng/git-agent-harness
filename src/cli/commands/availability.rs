// Command execution for `gah availability` (ticket #409).

use anyhow::Result;

use crate::cli::args::AvailabilityAction;

use crate::availability;

pub struct Args {
    pub json: bool,
    pub action: Option<AvailabilityAction>,
}

pub fn run(args: Args) -> Result<()> {
    match args.action {
        Some(AvailabilityAction::Clear {
            backend,
            backend_instance,
            model,
            quota_pool,
        }) => {
            availability::cli::clear(
                &availability::resolve_state_path(),
                &backend,
                backend_instance.as_deref(),
                model.as_deref(),
                quota_pool.as_deref(),
            )?;
            println!(
                "Marked backend '{backend}' available{}",
                model
                    .as_deref()
                    .map(|m| format!(" / model '{m}'"))
                    .unwrap_or_default()
            );
        }
        None => availability::cli::run(args.json)?,
    }
    Ok(())
}
