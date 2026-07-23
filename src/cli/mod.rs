// Library-owned CLI orchestration (ticket #406).
//
// `run()` is the single entry point that parses the command line and
// dispatches to the appropriate backend handler. The binary crate root
// (`src/main.rs`) only calls `git_agent_harness::cli::run()`; all parser
// definitions live in `crate::cli::args` (`src/cli/args.rs`).

use anyhow::Result;
use clap::Parser;

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
        Commands::Availability { json, action } => {
            commands::availability::run(commands::availability::Args { json, action })?
        }

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

        Commands::Ledger { command } => commands::ledger::run(command)?,

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
        } => commands::report::run(commands::report::Args {
            since,
            profile,
            config_path,
            group_by,
            json,
            series,
            bucket,
        })?,

        Commands::Server { port, host } => {
            println!("Starting WebSocket server on {}:{}", host, port);
            server::run_blocking(&host, port)?;
            // This will run forever, so we don't need to return
            std::thread::park();
        }
        Commands::Telemetry { command } => commands::telemetry::run(command)?,
        Commands::Quota { command } => commands::quota::run(command)?,
        Commands::Claims { command } => commands::claims::run(command)?,
    }
    Ok(())
}
