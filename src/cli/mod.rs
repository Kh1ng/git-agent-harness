// Library-owned CLI orchestration (ticket #406).
//
// `run()` is the single entry point that parses the command line and
// dispatches to the appropriate backend handler. The binary crate root
// (`src/main.rs`) only calls `git_agent_harness::cli::run()`; all parser
// definitions live in `crate::cli::args` (`src/cli/args.rs`).

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

// Bring the crate-root modules into scope so the command handlers can call
// them exactly as they did from the binary crate root.
use crate::*;
// Bring the parser structs/enums and `parse_wake_autonomy` into scope.
use crate::cli::args::*;

use crate::update;

pub mod args;
pub mod commands;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Availability { json, action } => match action {
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
            None => availability::cli::run(json)?,
        },

        Commands::Candidates {
            gate_artifact,
            include_warnings,
            out_root,
        } => candidates::run(&gate_artifact, include_warnings, &out_root)?,

        Commands::PriceGuard { watchlist, model } => price_guard::run(&watchlist, &model)?,

        Commands::PolicyCheck { config, action } => policy::run(&config, &action)?,

        Commands::Doctor {
            profile,
            config_path,
            validate,
            json,
        } => commands::doctor::run(profile.as_deref(), config_path.as_deref(), validate, json)?,

        Commands::Update {
            repo,
            restart_server,
            server_service,
        } => update::run(update::UpdateArgs {
            repo,
            restart_server,
            server_service,
        })?,

        Commands::Init {
            profile,
            display_name,
            provider,
            repo,
            local_path,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            artifact_root,
            worktree_base,
            oh_profile,
            config_path,
            print,
        } => commands::init::run(init::InitArgs {
            profile,
            display_name,
            provider,
            repo,
            local_path,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            artifact_root,
            worktree_base,
            oh_profile,
            config_path,
            print,
        })?,

        Commands::Prune {
            dry_run,
            older_than,
            profile,
            config_path,
        } => prune::run(
            profile.as_deref(),
            config_path.as_deref(),
            older_than,
            dry_run,
        )?,

        Commands::Ledger { command } => match command {
            LedgerCommands::RepairTail {
                config_path,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let repaired = ledger::repair_truncated_tail(&cfg, dry_run)?;
                match repaired.backup_path {
                    Some(path) if dry_run => println!(
                        "Dry run: would back up and remove {} truncated bytes; backup path: {}",
                        repaired.dropped_bytes,
                        path.display()
                    ),
                    Some(path) => println!(
                        "Repaired ledger tail: backed up and removed {} truncated bytes at {}",
                        repaired.dropped_bytes,
                        path.display()
                    ),
                    None => println!("Ledger tail is complete; no repair needed."),
                }
            }
            LedgerCommands::Summary {
                since,
                profile,
                config_path,
                json,
                group_by,
            } => ledger::summary::run_with_json(
                &since,
                profile.as_deref(),
                config_path.as_deref(),
                json,
                group_by,
            )?,
            LedgerCommands::Reconcile {
                profile,
                config_path,
                json,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                ledger::reconcile::run(&cfg, &profile, json, dry_run)?;
            }
            LedgerCommands::Work {
                work_id,
                config_path,
                json,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let mut entries = ledger::entries_for_work_id(&cfg, &work_id)?;
                entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                if json {
                    println!("{}", serde_json::to_string(&entries)?);
                } else if entries.is_empty() {
                    println!("No ledger entries found for work item '{}'.", work_id);
                } else {
                    println!("Work item: {} ({} entries)", work_id, entries.len());
                    for e in &entries {
                        let cost = e
                            .usage
                            .actual_cost_usd
                            .or(e.usage.estimated_cost_usd)
                            .map(|c| format!("${c:.4}"))
                            .unwrap_or_else(|| "unknown cost".into());
                        println!(
                            "  {}  {}  {}/{}  validation={} failure={} duration={} {}",
                            e.timestamp,
                            e.mode,
                            e.effective_backend,
                            e.effective_model.as_deref().unwrap_or("?"),
                            e.validation_result.as_deref().unwrap_or("-"),
                            e.failure_class.as_deref().unwrap_or("-"),
                            e.duration_seconds
                                .map(|d| format!("{d:.0}s"))
                                .unwrap_or_else(|| "unknown".into()),
                            cost,
                        );
                    }
                }
            }
            LedgerCommands::ClearAttempts {
                profile,
                work_id,
                config_path,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let prof = config::get_profile(&cfg, &profile)?;
                let entry = ledger::LedgerEntry::new_clear_attempts(&profile, prof, &work_id);
                if dry_run {
                    println!(
                        "Dry run: would append tombstone entry for work_id '{}':",
                        work_id
                    );
                    println!("{}", serde_json::to_string_pretty(&entry)?);
                } else {
                    let path = ledger::append(&cfg, &entry)?;
                    println!(
                        "Appended tombstone entry for work_id '{}' to {}",
                        work_id,
                        path.display()
                    );
                }
            }
        },

        Commands::Hold { command } => commands::controller::run_hold(command)?,

        Commands::RouteApproval { command } => commands::controller::run_route_approval(command)?,

        Commands::Loop {
            profile,
            config_path,
            json,
            once,
            parallel,
            skip_validation_gate,
        } => commands::controller::run_loop(commands::controller::LoopArgs {
            profile,
            config_path,
            json,
            once,
            parallel,
            skip_validation_gate,
        })?,

        Commands::Events {
            config_path,
            profile,
            json,
            since,
        } => commands::controller::run_events(commands::controller::EventsArgs {
            config_path,
            profile,
            json,
            since,
        })?,

        Commands::Status {
            profile,
            json,
            config_path,
        } => commands::controller::run_status(commands::controller::StatusArgs {
            profile,
            json,
            config_path,
        })?,

        Commands::Sync {
            profile,
            config_path,
            json,
        } => commands::controller::run_sync(commands::controller::SyncArgs {
            profile,
            config_path,
            json,
        })?,

        Commands::Dispatch {
            profile,
            mode,
            backend,
            target,
            branch,
            mr,
            current_branch,
            budget,
            dry_run,
            config_path,
            model,
            oh_profile,
            retries,
            allow_draft_fail,
            prod,
            issue_intake_override,
            allow_unknown_red_baseline,
            escalate,
            existing_branch,
            skip_validation_gate,
        } => commands::dispatch::run(commands::dispatch::Args {
            profile,
            mode,
            backend,
            target,
            branch,
            mr,
            current_branch,
            budget,
            dry_run,
            config_path,
            oh_profile,
            model,
            retries,
            allow_draft_fail,
            prod,
            issue_intake_override,
            allow_unknown_red_baseline,
            escalate,
            existing_branch,
            skip_validation_gate,
        })?,

        Commands::Pm { command } => match command {
            PmCommands::Publish {
                profile,
                plan,
                config_path,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let resolved_config_path = config::resolve_config_path(config_path.as_deref());
                let _lock = controller::acquire_profile_lock(&profile, &resolved_config_path)?;
                dispatch::publish_pm_plan(&cfg, &profile, &plan, dry_run)?;
            }
        },

        Commands::Tui {
            profile,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            tui::run(&cfg, profile.as_deref())?;
        }

        Commands::Config { command } => commands::config::run(command)?,

        Commands::Profile { command } => commands::profile::run(*command)?,

        Commands::Report {
            since,
            profile,
            config_path,
            group_by,
            json,
            series,
            bucket,
        } => {
            report::run(report::ReportArgs {
                since,
                profile,
                config_path,
                group_by,
                json,
                series,
                bucket,
            })?;
        }

        Commands::Server { port, host } => {
            println!("Starting WebSocket server on {}:{}", host, port);
            server::run_blocking(&host, port)?;
            // This will run forever, so we don't need to return
            std::thread::park();
        }
        Commands::Telemetry { command } => match command {
            TelemetryCommands::Export {
                telemetry_repo_path,
                format,
                output,
                since,
                profile,
                group_by,
                generate_manifests,
                config_path,
            } => {
                let format_enum = format
                    .parse::<telemetry::exporter::ExportFormat>()
                    .map_err(|e| anyhow::anyhow!("Invalid format: {}", e))?;
                telemetry::cli::run_export(
                    telemetry_repo_path.as_deref(),
                    Some(format_enum),
                    output.as_deref(),
                    Some(&since),
                    profile.as_deref(),
                    Some(group_by),
                    generate_manifests,
                    config_path.as_deref(),
                )?;
            }
            TelemetryCommands::Status {
                telemetry_repo_path,
                config_path,
            } => {
                telemetry::cli::run_status(telemetry_repo_path.as_deref(), config_path.as_deref())?;
            }
            TelemetryCommands::Aggregate {
                dimensions,
                since,
                until,
                profile,
                include_failed,
                include_retried,
                json,
                config_path,
                project,
                ticket,
                execution_type,
                backend_instance,
                provider,
                model,
                account,
            } => {
                // Parse dimensions from strings to AggregationDimension enum
                let parsed_dimensions = dimensions
                    .iter()
                    .map(|dim| dim.parse::<telemetry::AggregationDimension>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| anyhow::anyhow!("Invalid dimension: {}", e))?;

                telemetry::cli::run_aggregate(
                    parsed_dimensions,
                    since.as_deref(),
                    until.as_deref(),
                    profile.as_deref(),
                    include_failed,
                    include_retried,
                    json,
                    config_path.as_deref(),
                    project.as_deref(),
                    ticket.as_deref(),
                    execution_type.as_deref(),
                    backend_instance.as_deref(),
                    provider.as_deref(),
                    model.as_deref(),
                    account.as_deref(),
                )?;
            }
        },
        Commands::Quota { command } => match command {
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
                    .map(PathBuf::from)
                    .unwrap_or_else(quota_store::store_path);
                if quota_pool.is_some() && backend_instance.is_none() {
                    anyhow::bail!("--quota-pool requires --backend-instance for an unambiguous quota observation");
                }
                let is_vibe_admin = crate::config::canonical_backend_name(&backend) == "vibe";
                if is_vibe_admin && backend_instance.is_some() {
                    anyhow::bail!(
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
                    identity.backend_instance = execution_identity::validate_secret_safe_label(
                        "backend instance",
                        &instance,
                    )?;
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
                    .map(PathBuf::from)
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
        },
        Commands::Claims { command } => match command {
            ClaimsCommands::List {
                json,
                profile,
                config_path,
            } => {
                let scope = profile
                    .map(|profile: String| {
                        work_claim::canonical_scope_for_profile(&profile, config_path.as_deref())
                    })
                    .transpose()?;
                work_claim::handle_claims_list(scope.as_deref(), json)?;
            }
            ClaimsCommands::Clear { work_id, profile } => {
                let scope = work_claim::canonical_scope_for_profile(&profile, None)?;
                work_claim::handle_claims_clear(&scope, &work_id)?;
                println!("Cleared claim for work_id {work_id} on profile {profile}");
            }
            ClaimsCommands::Reclaim {
                profile,
                max_age_secs,
            } => {
                let scope = work_claim::canonical_scope_for_profile(&profile, None)?;
                let reclaimed = work_claim::handle_claims_reclaim(&scope, max_age_secs)?;
                if reclaimed.is_empty() {
                    println!("No stale claims to reclaim for profile {}", profile);
                } else {
                    println!(
                        "Reclaimed {} stale claim(s) for profile {}: {}",
                        reclaimed.len(),
                        profile,
                        reclaimed.join(", ")
                    );
                }
            }
        },
    }
    Ok(())
}
