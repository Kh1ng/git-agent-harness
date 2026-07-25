// Command execution for `gah quota` (ticket #409).

use anyhow::{bail, Result};

use crate::cli::args::QuotaCommands;
use crate::{config, execution_identity, quota_snapshot, quota_store};

use serde_json;

pub fn run(command: QuotaCommands) -> Result<()> {
    match command {
        QuotaCommands::Refresh {
            backend,
            backend_instance,
            model,
            quota_pool,
            command: cmd,
            store_path: store_arg,
        } => {
            let codex_cmd = cmd.unwrap_or_else(|| backend.clone());
            let path = store_arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(quota_store::store_path);
            if quota_pool.is_some() && backend_instance.is_none() {
                bail!(
                    "--quota-pool requires --backend-instance for an unambiguous quota observation"
                );
            }
            let is_vibe_admin = crate::config::canonical_backend_name(&backend) == "vibe";
            if is_vibe_admin && backend_instance.is_some() {
                bail!(
                    "--backend-instance is not supported for --backend vibe: the Mistral Admin API key is a single org-wide credential, not a per-instance one"
                );
            }

            let refreshed = if is_vibe_admin {
                quota_store::refresh_vibe_admin_and_store(model.as_deref(), &path)
            } else if let Some(instance) = backend_instance {
                let mut identity = execution_identity::ExecutionIdentity::legacy_candidate(
                    &backend,
                    model.as_deref(),
                    quota_pool.as_deref(),
                );
                identity.backend_instance =
                    execution_identity::validate_secret_safe_label("backend instance", &instance)?;
                quota_store::refresh_codex_and_store_for_identity(&codex_cmd, &identity, &path)
            } else {
                quota_store::refresh_codex_and_store(&codex_cmd, model.as_deref(), &path)
            };

            match refreshed {
                Ok(Some(rec)) => {
                    if is_vibe_admin && rec.quota_used_percent.is_none() {
                        println!(
                            "Recorded Mistral Admin account data without a spend-limit reading (workspace/billing/rate-limit data saved; nothing fabricated)."
                        );
                    } else {
                        println!(
                            "Refreshed {} {} quota: used={:?}% remaining={:?}% window={:?} reset={:?} (source={})",
                            rec.backend,
                            rec.model.as_deref().unwrap_or(""),
                            rec.quota_used_percent,
                            rec.quota_remaining_percent,
                            rec.quota_window,
                            rec.quota_reset_at,
                            rec.usage_source.as_deref().unwrap_or(""),
                        );
                    }
                }
                Ok(None) if is_vibe_admin => {
                    println!(
                        "No account-level quota data from the Mistral Admin API (missing MISTRAL_ADMIN_API_KEY or unreachable; nothing fabricated)."
                    );
                }
                Ok(None) => {
                    println!(
                        "No account-level quota data from `{} status --json` (ok: nothing fabricated).",
                        codex_cmd
                    );
                }
                Err(e) => {
                    eprintln!("Quota refresh failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        QuotaCommands::List {
            json,
            store_path: store_arg,
        } => {
            let path = store_arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(quota_store::store_path);
            let records = quota_store::load(&path).unwrap_or_default();
            if json {
                println!("{}", serde_json::to_string(&records)?);
            } else if records.is_empty() {
                println!("No persisted quota observations.");
            } else {
                for rec in &records {
                    println!(
                        "{} {}/{}: used={:?}% remaining={:?}% window={:?} reset={:?} ({})",
                        rec.observed_at.as_deref().unwrap_or(""),
                        rec.backend,
                        rec.model.as_deref().unwrap_or(""),
                        rec.quota_used_percent,
                        rec.quota_remaining_percent,
                        rec.quota_window,
                        rec.quota_reset_at,
                        rec.usage_source.as_deref().unwrap_or(""),
                    );
                }
            }
        }
        QuotaCommands::Snapshot {
            profile,
            since,
            json,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            quota_snapshot::run(&cfg, &profile, &since, json)?;
        }
    }

    Ok(())
}
