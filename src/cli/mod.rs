// Library-owned CLI orchestration (ticket #406).
//
// `run()` is the single entry point that parses the command line and
// dispatches to the appropriate backend handler. The binary crate root
// (`src/main.rs`) only calls `git_agent_harness::cli::run()`; all parser
// definitions live in `crate::cli::args` (`src/cli/args.rs`).

use anyhow::Result;
use clap::Parser;

// Bring the parser structs/enums and `parse_wake_autonomy` into scope.
use crate::cli::args::*;
use crate::init;

pub mod args;
pub mod capabilities;
pub mod commands;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Memory(args) => commands::memory::run(args)?,
        Commands::Route(args) => commands::route::run(
            &args.profile,
            args.model.as_deref(),
            args.failure_backend.as_deref(),
            args.backend_instance.as_deref(),
            args.config_path.as_deref(),
        )?,
        Commands::Map(args) => crate::planning::run(args)?,
        Commands::Installer(command) => crate::installer::run(command)?,
        Commands::Setup {
            command: Some(command),
            ..
        } => commands::setup::run(command)?,
        Commands::Setup {
            command: None,
            args,
        } => crate::setup::run(args)?,
        Commands::Availability { json, action } => {
            commands::availability::run(commands::availability::Args { json, action })?
        }

        Commands::Candidates {
            gate_artifact,
            include_warnings,
            out_root,
        } => commands::candidates::run(commands::candidates::Args {
            gate_artifact,
            include_warnings,
            out_root,
        })?,

        Commands::PriceGuard { watchlist, model } => {
            commands::price_guard::run(commands::price_guard::Args { watchlist, model })?
        }

        Commands::NotifySend {
            config_path,
            title,
            message,
            url,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let (channel, outcome) =
                crate::notify_channels::send_activity(&cfg, &title, &message, url.as_deref());
            println!(
                "{}",
                crate::notify_channels::activity_receipt_line(channel, &outcome)
            );
            outcome?
        }

        Commands::AuthHealth { classify } => match classify {
            Some(backend) => {
                let mut text = String::new();
                std::io::Read::read_to_string(
                    &mut std::io::Read::take(std::io::stdin(), 64 * 1024),
                    &mut text,
                )?;
                println!(
                    "{}",
                    serde_json::json!({ "auth_failure": crate::auth_health::is_auth_failure(&backend, &text) })
                );
            }
            None => println!(
                "{}",
                serde_json::to_string(&crate::auth_health::probe_node())?
            ),
        },

        Commands::PolicyCheck { config, action } => {
            commands::policy::run(commands::policy::Args { config, action })?
        }

        Commands::Doctor {
            profile,
            config_path,
            validate,
            json,
        } => commands::doctor::run(profile.as_deref(), config_path.as_deref(), validate, json)?,

        Commands::Update(args) => commands::update::run(args)?,

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
        } => commands::prune::run(commands::prune::Args {
            profile,
            config_path,
            older_than,
            dry_run,
        })?,

        Commands::NetworkExpose {
            port,
            label,
            level,
            config_path,
        } => commands::network::run(commands::network::Args {
            port,
            label,
            level,
            config_path,
        })?,

        Commands::TailscaleIp { config_path, json } => {
            commands::network::tailscale_ip(commands::network::TailscaleIpArgs {
                config_path,
                json,
            })?
        }

        Commands::WatchdogCheck {
            profile,
            config_path,
        } => commands::watchdog::run(commands::watchdog::Args {
            profile,
            config_path,
        })?,

        Commands::Ledger { command } => commands::ledger::run(command)?,

        Commands::Hold { command } => commands::controller::run_hold(command)?,

        Commands::RouteApproval { command } => commands::controller::run_route_approval(command)?,

        Commands::ExternalApproval { command } => commands::external_approval::run(command)?,

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
            role,
            light,
            json,
            config_path,
        } => commands::controller::run_status(commands::controller::StatusArgs {
            profile,
            role,
            light,
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
            enforce_job_file,
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
            enforce_job_file,
        })?,

        Commands::Pm { command } => commands::pm::run(command)?,

        Commands::Tui {
            profile,
            config_path,
        } => commands::tui::run(commands::tui::Args {
            profile,
            config_path,
        })?,

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
            commands::server::run(commands::server::Args { port, host })?
        }
        Commands::Telemetry { command } => commands::telemetry::run(command)?,
        Commands::Quota { command } => commands::quota::run(command)?,
        Commands::Credentials { command } => commands::credentials::run(command)?,
        Commands::Skills { command } => commands::skills::run(command)?,
        Commands::Claims { command } => commands::claims::run(command)?,
        Commands::Node { command } => commands::node::run(command)?,
    }
    Ok(())
}
