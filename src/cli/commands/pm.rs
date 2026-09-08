//! PM CLI presentation. Remote IDs share the same locked publisher as local paths.
use crate::cli::args::PmCommands;
use crate::{config, controller, dispatch};
use anyhow::Result;

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let mut value = serde_json::to_value(value)?;
    crate::redact::redact_json_value(&mut value);
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub fn run(command: PmCommands) -> Result<()> {
    match command {
        PmCommands::Plans {
            profile,
            cursor,
            limit,
            config_path,
            json: _,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            print_json(&dispatch::pm_plans::list(
                &cfg,
                &profile,
                cursor.as_deref(),
                limit as usize,
            )?)
        }
        PmCommands::Show {
            profile,
            plan_id,
            config_path,
            json: _,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            print_json(&dispatch::pm_plans::show(&cfg, &profile, &plan_id)?)
        }
        PmCommands::Publish {
            profile,
            plan,
            plan_id,
            expected_fingerprint,
            config_path,
            dry_run,
            json,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let resolved = config::resolve_config_path(config_path.as_deref());
            let _lock = controller::acquire_profile_lock(&profile, &resolved)?;
            if let Some(id) = plan_id {
                let result = dispatch::pm_plans::publish(
                    &cfg,
                    &profile,
                    &id,
                    expected_fingerprint.as_deref(),
                    dry_run,
                )?;
                if json {
                    print_json(&result)?;
                } else {
                    for line in result.output {
                        println!("{line}");
                    }
                    if let Some(error) = result.error {
                        anyhow::bail!("{error}");
                    }
                }
            } else {
                let path =
                    plan.ok_or_else(|| anyhow::anyhow!("--plan or --plan-id is required"))?;
                let result = dispatch::publish_pm_plan(&cfg, &profile, &path, dry_run, None)?;
                for line in result.output {
                    println!("{line}");
                }
            }
            Ok(())
        }
    }
}
