// Command execution for `gah quota` (ticket #409).

use anyhow::{bail, Result};

use crate::cli::args::QuotaCommands;
use crate::{config, execution_identity, quota_snapshot, quota_store};

use serde_json;

pub fn run(command: QuotaCommands) -> Result<()> {
    match command {
        QuotaCommands::Record { store_path } => {
            use std::io::Read;
            let mut input = String::new();
            std::io::stdin()
                .take(16 * 1024 + 1)
                .read_to_string(&mut input)?;
            if input.len() > 16 * 1024 {
                bail!("quota observation exceeds 16 KiB");
            }
            let record = quota_store::parse_external_observation(&input)?;
            let path = store_path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(quota_store::store_path);
            quota_store::append(&path, &record)?;
            println!("Recorded account-level quota observation.");
        }
        QuotaCommands::Refresh {
            credential,
            backend,
            backend_instance,
            model,
            quota_pool,
            command: cmd,
            store_path: store_arg,
        } => {
            if let Some(id) = credential {
                if backend != "codex"
                    || backend_instance.is_some()
                    || model.is_some()
                    || quota_pool.is_some()
                    || cmd.is_some()
                {
                    bail!("--credential selects its own provider; backend/instance/model/pool/command overrides are unsupported");
                }
                let path = store_arg
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(quota_store::store_path);
                let record = crate::credentials::quota::refresh(&id, &path)?;
                if let Some(error) = record.check_error {
                    bail!("Quota refresh failed: {error}");
                }
                println!("Recorded named provider account check.");
                return Ok(());
            }
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
            if backend == "mistral-dashboard"
                && (backend_instance.is_some()
                    || model.is_some()
                    || quota_pool.is_some()
                    || codex_cmd != "mistral-dashboard")
            {
                bail!("Mistral dashboard refresh uses the owner-selected cookie's current account; command/instance/model/pool overrides are unsupported");
            }
            if backend == "claude"
                && (backend_instance.is_some()
                    || model.is_some()
                    || quota_pool.is_some()
                    || codex_cmd != "claude")
            {
                bail!("Claude quota refresh uses the current native OAuth login; command/instance/model/pool overrides are unsupported");
            }
            if backend == "nous"
                && (backend_instance.is_some() || model.is_some() || quota_pool.is_some())
            {
                bail!("Nous account refresh uses the configured nous-portal-api account; instance/model/pool overrides are unsupported");
            }
            if is_vibe_admin && backend_instance.is_some() {
                bail!(
                    "--backend-instance is not supported for --backend vibe: the Mistral Admin API key is a single org-wide credential, not a per-instance one"
                );
            }

            let refreshed = if backend == "claude" {
                quota_store::refresh_claude_and_store(&path)
            } else if backend == "agy" {
                if backend_instance.is_some() || model.is_some() || quota_pool.is_some() {
                    bail!("Antigravity usage reports its own model pools and windows; instance/model/pool overrides are unsupported");
                }
                quota_store::refresh_agy_and_store(&codex_cmd, &backend, None, &path)
            } else if backend == "nous" {
                let record = crate::usage::nous::refresh()?;
                quota_store::append(&path, &record)?;
                Ok(Some(record))
            } else if backend == "mistral-dashboard" {
                crate::usage::mistral_dashboard::refresh_and_store(&path)
            } else if is_vibe_admin {
                quota_store::refresh_vibe_admin_and_store(model.as_deref(), &path)
            } else if let Some(instance) = backend_instance {
                let mut identity = execution_identity::ExecutionIdentity::legacy_candidate(
                    &backend,
                    model.as_deref(),
                    quota_pool.as_deref(),
                );
                identity.backend_instance =
                    execution_identity::validate_operator_label("backend instance", &instance)?;
                quota_store::refresh_codex_and_store(
                    &codex_cmd,
                    model.as_deref(),
                    Some(&identity),
                    &[],
                    &path,
                )
            } else {
                quota_store::refresh_codex_and_store(&codex_cmd, model.as_deref(), None, &[], &path)
            };

            match refreshed {
                Ok(Some(rec)) => {
                    if is_vibe_admin && rec.quota_used_percent.is_none() {
                        println!(
                            "Recorded Mistral Admin account data without a spend-limit reading (workspace/billing/rate-limit data saved; nothing fabricated)."
                        );
                    } else {
                        println!("Refreshed account-level quota data.");
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
        QuotaCommands::AutoRefresh {
            store_path: store_arg,
        } => {
            // Issue #761: periodic, unattended account-level quota refresh.
            // Iterate every configured profile so per-profile codex_path
            // overrides are honored, and let refresh_quota_observations_and_wait
            // apply the per-source 14-min throttle + bounded supervision.
            // The store is shared across profiles, so one store path feeds all.
            let path = store_arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(quota_store::store_path);
            // A node with no config yet has no profiles to refresh -- report
            // and exit cleanly rather than failing the systemd oneshot timer.
            let cfg = match config::load(None) {
                Ok(cfg) => cfg,
                Err(error) => {
                    println!("No gah config to refresh quota for (skipping): {error}");
                    return Ok(());
                }
            };
            let mut names: Vec<&String> = cfg.profiles.keys().collect();
            names.sort();
            let mut refreshed_any = false;
            for name in names {
                let mut profile = cfg.profiles[name].clone();
                profile.routing = profile.effective_routing(&cfg.defaults);
                let refreshed = quota_store::refresh_quota_observations_and_wait(
                    &profile,
                    time::OffsetDateTime::now_utc(),
                    &path,
                );
                if refreshed > 0 {
                    println!("Refreshed {refreshed} quota backend(s) for profile '{name}'");
                    refreshed_any = true;
                }
            }
            if !refreshed_any {
                println!("No quota backends were due for refresh.");
            }
        }
        QuotaCommands::List {
            json,
            store_path: store_arg,
        } => {
            let path = store_arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(quota_store::store_path);
            let records = quota_store::load(&path)?;
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
