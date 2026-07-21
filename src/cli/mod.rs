// Library-owned CLI orchestration (ticket #406).
//
// `run()` is the single entry point that parses the command line and
// dispatches to the appropriate backend handler. The binary crate root
// (`src/main.rs`) only calls `git_agent_harness::cli::run()`; all parser
// definitions live in `crate::cli::args` (`src/cli/args.rs`).

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use uuid::Uuid;

// Bring the crate-root modules into scope so the command handlers can call
// them exactly as they did from the binary crate root.
use crate::*;
// Bring the parser structs/enums and `parse_wake_autonomy` into scope.
use crate::cli::args::*;

use crate::config::Profile;
use crate::update;

pub mod args;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Availability { json, action } => match action {
            Some(AvailabilityAction::Clear {
                backend,
                model,
                quota_pool,
            }) => {
                availability::cli::clear(
                    &availability::resolve_state_path(),
                    &backend,
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
        } => doctor::run(profile.as_deref(), config_path.as_deref(), validate, json)?,

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
        } => init::run(init::InitArgs {
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

        Commands::Hold { command } => match command {
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
        },

        Commands::RouteApproval { command } => {
            let (profile, work_id, backend, model, config_path, granted) = match command {
                RouteApprovalCommands::Grant {
                    profile,
                    work_id,
                    backend,
                    model,
                    config_path,
                } => (profile, work_id, backend, model, config_path, true),
                RouteApprovalCommands::Revoke {
                    profile,
                    work_id,
                    backend,
                    model,
                    config_path,
                } => (profile, work_id, backend, model, config_path, false),
            };
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_paid_route_approval(
                &profile,
                prof,
                &work_id,
                &backend,
                model.as_deref(),
                granted,
            );
            let path = ledger::append(&cfg, &entry)?;
            println!(
                "Paid route approval {} for work_id '{}' on {}/{} ({})",
                if granted { "granted" } else { "revoked" },
                work_id,
                backend,
                model.as_deref().unwrap_or("default"),
                path.display()
            );
        }

        Commands::Loop {
            profile,
            config_path,
            json,
            once,
            parallel,
            skip_validation_gate,
        } => {
            runner::install_shutdown_handler()?;
            let cfg = config::load(config_path.as_deref())?;
            let resolved_config_path = config::resolve_config_path(config_path.as_deref());
            let parallel = controller::loop_parallel_argument(
                once,
                parallel,
                config::get_profile(&cfg, &profile)?.max_parallel_workers() as usize,
            );
            if once {
                // `--once` still does real execution (spawns backends, claims
                // tickets, writes ledger entries) so it must coordinate via
                // the same profile lock as the daemon (`gah loop` with no
                // `--once`) -- otherwise both can run concurrently against
                // the same profile. `run_loop` acquires this lock itself for
                // the daemon case; do not acquire it again from within
                // `run_once` itself, only here at the entry point.
                let _lock = controller::acquire_profile_lock(&profile, &resolved_config_path)?;
                controller::run_once(&cfg, &profile, json, parallel, skip_validation_gate)?;
            } else {
                controller::run_loop(
                    &cfg,
                    &profile,
                    json,
                    parallel,
                    skip_validation_gate,
                    &resolved_config_path,
                )?;
            }
        }

        Commands::Events {
            config_path,
            profile,
            json,
            since,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            events::run(&cfg, &since, profile.as_deref(), json)?;
        }

        Commands::Status {
            profile,
            json,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            status::run(&cfg, &profile, json)?;
        }

        Commands::Sync {
            profile,
            config_path,
            json,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            sync::run(&cfg, &profile, json)?;
        }

        Commands::Dispatch {
            profile,
            mode,
            backend,
            target,
            branch,
            mr,
            current_branch,
            budget: _budget,
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
        } => {
            runner::install_shutdown_handler()?;
            let cfg = config::load(config_path.as_deref())?;
            let run_id = Uuid::new_v4().to_string();
            let resolved_config_path = config::resolve_config_path(config_path.as_deref());
            let _lock = controller::acquire_profile_lock(&profile, &resolved_config_path)?;
            let args = dispatch::DispatchArgs {
                profile,
                mode,
                backend,
                target,
                branch,
                mr,
                current_branch,
                dry_run,
                model,
                oh_profile,
                retries,
                allow_draft_fail,
                prod,
                issue_intake_override,
                allow_unknown_red_baseline,
                escalate,
                existing_branch,
                expected_review_generation: None,
                skip_validation_gate,
                dispatch_reason: None,
                work_id: None,
                run_id: Some(run_id),
                route_ready: None,
            };
            controller::run_dispatch_and_record(&cfg, "dispatch", None, &args)?;
        }

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

        Commands::Config { command } => match command {
            ConfigCommands::Show {
                json,
                full,
                profile,
                config_path,
            } => {
                let resolved_config_path = config::resolve_config_path(config_path.as_deref());
                let cfg = config::load(config_path.as_deref())?;
                if json {
                    if full {
                        println!(
                            "{}",
                            config_show::config_show_full_json(
                                &cfg,
                                &resolved_config_path,
                                profile.as_deref(),
                            )?
                        );
                    } else {
                        // Compatibility contract: bare `config show --json`
                        // remains byte-for-byte the original one-field shape.
                        println!("{}", config_show::config_show_json(&cfg)?);
                    }
                } else {
                    println!(
                        "current_manager: {}",
                        cfg.defaults.current_manager.as_deref().unwrap_or("(unset)")
                    );
                }
            }
            ConfigCommands::Set {
                config_path,
                current_manager,
                clear,
            } => {
                let mut cfg = config::load(config_path.as_deref())?;
                if let Some(v) = current_manager {
                    cfg.defaults.current_manager = Some(v);
                } else if clear.contains(&"current_manager".to_string()) {
                    cfg.defaults.current_manager = None;
                }
                config::save(&cfg, config_path.as_deref())?;
                println!("Updated global config");
            }
        },

        Commands::Profile { command } => match *command {
            ProfileCommands::List { config, json } => {
                let cfg = config::load(config.as_deref())?;
                let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
                names.sort_unstable();
                if json {
                    println!("{}", profile_output::list_json(&cfg)?);
                } else {
                    for name in names {
                        let p = &cfg.profiles[name];
                        println!("{:<25} {} ({})", name, p.display_name, p.provider);
                    }
                }
            }
            ProfileCommands::Show { name, config } => {
                let cfg = config::load(config.as_deref())?;
                let p = config::get_profile(&cfg, &name)?;
                println!("name:                  {}", name);
                println!("display_name:          {}", p.display_name);
                println!("repo_id:               {}", p.repo_id);
                println!("provider:              {}", p.provider);
                println!("repo:                  {}", p.repo);
                println!("local_path:            {}", p.local_path);
                println!("artifact_root:         {}", p.artifact_root);
                println!("default_target_branch: {}", p.default_target_branch);
                if let Some(api) = &p.provider_api_base {
                    println!("provider_api_base:     {}", api);
                }
                if let Some(id) = &p.provider_project_id {
                    println!("provider_project_id:   {}", id);
                }
                if !p.openhands_args.is_empty() {
                    println!("openhands_args:        {:?}", p.openhands_args);
                }
                if !p.codex_args.is_empty() {
                    println!("codex_args:            {:?}", p.codex_args);
                }
                if !p.claude_args.is_empty() {
                    println!("claude_args:           {:?}", p.claude_args);
                }
                if !p.validation_commands.is_empty() {
                    println!("validation_commands:");
                    for cmd in &p.validation_commands {
                        println!("  - {}", cmd);
                    }
                }
                println!(
                    "validation_timeout_seconds: {}",
                    p.validation_timeout_seconds()
                );
            }
            ProfileCommands::Add {
                name,
                display_name,
                repo_id,
                provider,
                repo,
                local_path,
                artifact_root,
                default_target_branch,
                provider_api_base,
                provider_project_id,
                config_path,
                openhands_args,
                codex_args,
                codex_path,
                claude_args,
                claude_path,
                agy_path,
                vibe_args,
                vibe_path,
                opencode_args,
                opencode_path,
                agy_second_home,
                notify_command,
                policy_path,
                env_file,
                env_file_prod,
                validation_commands,
                auto_fix_commands,
                max_parallel_workers,
                max_open_managed_mrs,
                validation_timeout_seconds,
                manager_wake_autonomy,
                delivery_mode,
            } => {
                let mut cfg = config::load(config_path.as_deref())?;
                let profile = Profile {
                    display_name,
                    repo_id,
                    provider,
                    repo,
                    local_path,
                    artifact_root,
                    default_target_branch,
                    provider_api_base,
                    provider_project_id,
                    oh_profile: None,
                    openhands_args,
                    codex_args,
                    codex_path,
                    claude_args,
                    claude_path,
                    agy_path,
                    vibe_args,
                    vibe_path,
                    opencode_args,
                    opencode_path,
                    agy_second_home,
                    agy_print_timeout_seconds: std::collections::HashMap::new(),
                    agy_idle_timeout_seconds: None,
                    opencode_idle_timeout_seconds: None,
                    opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
                    max_concurrent_per_model: std::collections::HashMap::new(),
                    openhands_idle_timeout_seconds: None,
                    vibe_idle_timeout_seconds: None,
                    codex_idle_timeout_seconds: None,
                    claude_idle_timeout_seconds: None,
                    max_parallel_workers,
                    max_open_managed_mrs,
                    notify_command,
                    manager_wake_autonomy: match &manager_wake_autonomy {
                        Some(v) => parse_wake_autonomy(v)?,
                        None => config::WakeAutonomy::default(),
                    },
                    delivery_mode: match &delivery_mode {
                        Some(v) => parse_delivery_mode(v)?,
                        None => config::DeliveryMode::default(),
                    },
                    policy_path,
                    env_file,
                    env_file_prod,
                    validation_commands,
                    auto_fix_commands,
                    test_file_patterns: vec![],
                    known_baseline_failure_markers: vec![],
                    model_improve: None,
                    model_pm: None,
                    model_review: None,
                    review_timeout_seconds: None,
                    review_hard_timeout_seconds: None,
                    validation_timeout_seconds,
                    routing: config::RoutingPolicy::default(),
                    publishing: Default::default(),
                    pacing: Default::default(),
                    prune_older_than_days: None,
                };
                config::add_profile(&mut cfg, &name, profile)?;
                config::save(&cfg, config_path.as_deref())?;
                println!("Added profile '{}'", name);
            }
            ProfileCommands::Set {
                name,
                display_name,
                repo_id,
                provider,
                repo,
                local_path,
                artifact_root,
                default_target_branch,
                provider_api_base,
                provider_project_id,
                config_path,
                openhands_args,
                codex_args,
                codex_path,
                claude_args,
                claude_path,
                agy_path,
                vibe_args,
                vibe_path,
                opencode_args,
                opencode_path,
                agy_second_home,
                notify_command,
                policy_path,
                env_file,
                env_file_prod,
                validation_commands,
                auto_fix_commands,
                max_parallel_workers,
                max_open_managed_mrs,
                validation_timeout_seconds,
                manager_wake_autonomy,
                delivery_mode,
                clear,
            } => {
                let mut cfg = config::load(config_path.as_deref())?;
                let existing = config::get_profile_mut(&mut cfg, &name)?;

                // Helper to clear a field if requested
                fn should_clear(field: &str, clear_list: &[String]) -> bool {
                    clear_list.contains(&field.to_string())
                }

                // Update fields if provided, or clear if requested
                if let Some(v) = display_name {
                    existing.display_name = v;
                } else if should_clear("display_name", &clear) {
                    // display_name is required, so we don't actually clear it
                }

                if let Some(v) = repo_id {
                    existing.repo_id = v;
                } else if should_clear("repo_id", &clear) {
                    // repo_id is required
                }

                if let Some(v) = provider {
                    existing.provider = v;
                } else if should_clear("provider", &clear) {
                    // provider is required
                }

                if let Some(v) = repo {
                    existing.repo = v;
                } else if should_clear("repo", &clear) {
                    // repo is required
                }

                if let Some(v) = local_path {
                    existing.local_path = v;
                } else if should_clear("local_path", &clear) {
                    // local_path is required
                }

                if let Some(v) = artifact_root {
                    existing.artifact_root = v;
                } else if should_clear("artifact_root", &clear) {
                    // artifact_root is required
                }

                if let Some(v) = default_target_branch {
                    existing.default_target_branch = v;
                } else if should_clear("default_target_branch", &clear) {
                    // default_target_branch is required
                }

                if let Some(v) = provider_api_base {
                    existing.provider_api_base = Some(v);
                } else if should_clear("provider_api_base", &clear) {
                    existing.provider_api_base = None;
                }

                if let Some(v) = provider_project_id {
                    existing.provider_project_id = Some(v);
                } else if should_clear("provider_project_id", &clear) {
                    existing.provider_project_id = None;
                }

                if !openhands_args.is_empty() {
                    existing.openhands_args = openhands_args;
                } else if should_clear("openhands_args", &clear) {
                    existing.openhands_args.clear();
                }

                if !codex_args.is_empty() {
                    existing.codex_args = codex_args;
                } else if should_clear("codex_args", &clear) {
                    existing.codex_args.clear();
                }

                if let Some(v) = codex_path {
                    existing.codex_path = Some(v);
                } else if should_clear("codex_path", &clear) {
                    existing.codex_path = None;
                }

                if !claude_args.is_empty() {
                    existing.claude_args = claude_args;
                } else if should_clear("claude_args", &clear) {
                    existing.claude_args.clear();
                }

                if let Some(v) = claude_path {
                    existing.claude_path = Some(v);
                } else if should_clear("claude_path", &clear) {
                    existing.claude_path = None;
                }

                if let Some(v) = agy_path {
                    existing.agy_path = Some(v);
                } else if should_clear("agy_path", &clear) {
                    existing.agy_path = None;
                }

                if !vibe_args.is_empty() {
                    existing.vibe_args = vibe_args;
                } else if should_clear("vibe_args", &clear) {
                    existing.vibe_args.clear();
                }

                if let Some(v) = vibe_path {
                    existing.vibe_path = Some(v);
                } else if should_clear("vibe_path", &clear) {
                    existing.vibe_path = None;
                }

                if !opencode_args.is_empty() {
                    existing.opencode_args = opencode_args;
                } else if should_clear("opencode_args", &clear) {
                    existing.opencode_args.clear();
                }

                if let Some(v) = opencode_path {
                    existing.opencode_path = Some(v);
                } else if should_clear("opencode_path", &clear) {
                    existing.opencode_path = None;
                }

                if let Some(v) = agy_second_home {
                    existing.agy_second_home = Some(v);
                } else if should_clear("agy_second_home", &clear) {
                    existing.agy_second_home = None;
                }

                if let Some(v) = notify_command {
                    existing.notify_command = Some(v);
                } else if should_clear("notify_command", &clear) {
                    existing.notify_command = None;
                }

                if let Some(v) = policy_path {
                    existing.policy_path = Some(v);
                } else if should_clear("policy_path", &clear) {
                    existing.policy_path = None;
                }

                if let Some(v) = env_file {
                    existing.env_file = Some(v);
                } else if should_clear("env_file", &clear) {
                    existing.env_file = None;
                }

                if let Some(v) = env_file_prod {
                    existing.env_file_prod = Some(v);
                } else if should_clear("env_file_prod", &clear) {
                    existing.env_file_prod = None;
                }

                if !validation_commands.is_empty() {
                    existing.validation_commands = validation_commands;
                } else if should_clear("validation_commands", &clear) {
                    existing.validation_commands.clear();
                }

                if !auto_fix_commands.is_empty() {
                    existing.auto_fix_commands = auto_fix_commands;
                } else if should_clear("auto_fix_commands", &clear) {
                    existing.auto_fix_commands.clear();
                }

                if let Some(v) = max_parallel_workers {
                    existing.max_parallel_workers = Some(v);
                } else if should_clear("max_parallel_workers", &clear) {
                    existing.max_parallel_workers = None;
                }

                if let Some(v) = max_open_managed_mrs {
                    existing.max_open_managed_mrs = Some(v);
                } else if should_clear("max_open_managed_mrs", &clear) {
                    existing.max_open_managed_mrs = None;
                }

                if let Some(v) = validation_timeout_seconds {
                    existing.validation_timeout_seconds = Some(v);
                } else if should_clear("validation_timeout_seconds", &clear) {
                    existing.validation_timeout_seconds = None;
                }

                if let Some(v) = &manager_wake_autonomy {
                    existing.manager_wake_autonomy = parse_wake_autonomy(v)?;
                } else if should_clear("manager_wake_autonomy", &clear) {
                    existing.manager_wake_autonomy = config::WakeAutonomy::default();
                }

                if let Some(v) = &delivery_mode {
                    existing.delivery_mode = parse_delivery_mode(v)?;
                } else if should_clear("delivery_mode", &clear) {
                    existing.delivery_mode = config::DeliveryMode::default();
                }

                config::save(&cfg, config_path.as_deref())?;
                println!("Updated profile '{}'", name);
            }
            ProfileCommands::Remove {
                name,
                config_path,
                force,
            } => {
                if !force {
                    eprintln!("Warning: Removing profile '{}' cannot be undone.", name);
                    eprintln!("Use --force to confirm.");
                    anyhow::bail!("Aborted");
                }
                let mut cfg = config::load(config_path.as_deref())?;
                config::remove_profile(&mut cfg, &name)?;
                config::save(&cfg, config_path.as_deref())?;
                println!("Removed profile '{}'", name);
            }
        },

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
                model,
                command: cmd,
                store_path: store_arg,
            } => {
                let codex_cmd = cmd.unwrap_or_else(|| backend.clone());
                let path = store_arg
                    .map(PathBuf::from)
                    .unwrap_or_else(quota_store::store_path);
                match quota_store::refresh_codex_and_store(&codex_cmd, model.as_deref(), &path) {
                    Ok(Some(rec)) => {
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
