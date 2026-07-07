mod availability;
mod baseline;
mod candidates;
mod capability;
mod config;
mod controller;
mod dispatch;
mod doctor;
mod events;
mod init;
mod ledger;
mod models;
mod notifications;
mod policy;
mod price_guard;
mod provider;
mod prune;
mod quota;
mod quota_parser;
mod routing;
mod runner;
mod server;
mod status;
mod sync;
#[cfg(test)]
mod test_support;
mod tui;
mod tui_state;
mod usage;
mod worktree;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gah", about = "git agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show durable backend/model availability state (global, not per-profile)
    Availability {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Convert gate findings into backlog candidates
    Candidates {
        #[arg(long)]
        gate_artifact: String,
        #[arg(long, default_value_t = false)]
        include_warnings: bool,
        #[arg(long)]
        out_root: String,
    },
    /// Check model price against watchlist policy
    PriceGuard {
        #[arg(long)]
        watchlist: String,
        #[arg(long)]
        model: String,
    },
    /// Check repo policy for a given action
    PolicyCheck {
        #[arg(long)]
        config: String,
        #[arg(long)]
        action: String,
    },
    /// Validate config and profile setup
    Doctor {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        /// Also verify execution prerequisites: validation commands resolve,
        /// declared env files exist, backend executables are present.
        #[arg(long)]
        validate: bool,
    },
    /// Create or print a starter GAH config/profile
    Init {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        local_path: String,
        #[arg(long, default_value = "main")]
        default_target_branch: String,
        #[arg(long)]
        provider_api_base: Option<String>,
        #[arg(long)]
        provider_project_id: Option<String>,
        #[arg(long)]
        artifact_root: Option<String>,
        #[arg(long)]
        worktree_base: Option<String>,
        #[arg(long)]
        oh_profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        print: bool,
    },
    /// Delete old GAH-owned sessions and worktrees
    Prune {
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = 30)]
        older_than: u64,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Inspect ledger data
    Ledger {
        #[command(subcommand)]
        command: LedgerCommands,
    },
    /// One bounded controller iteration: observe, decide one action,
    /// execute at most that action, persist, exit. No daemon.
    Loop {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Required -- this is the only supported mode; named explicitly
        /// so a future recurring mode can't be silently assumed.
        #[arg(long, default_value_t = false)]
        once: bool,
    },
    /// Inspect the controller event stream (TICKET-083)
    Events {
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, default_value = "7d")]
        since: String,
    },
    /// Provide a single machine-readable controller snapshot of all state
    Status {
        #[arg(long)]
        profile: String,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Classify open GAH-created merge requests / pull requests
    Sync {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Dispatch a job to a backend (improve, pm, review, fix, experiment)
    Dispatch {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        mode: String,
        /// Backend: openhands, cloud-coder, codex, claude, agy, vibe, auto
        #[arg(long, default_value = "auto")]
        backend: String,
        #[arg(long, default_value = "")]
        target: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        mr: Option<String>,
        #[arg(long, default_value_t = false)]
        current_branch: bool,
        #[arg(long, default_value = "1")]
        budget: u32,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Path to gah-config.toml (default: auto-discovered)
        #[arg(long, name = "config")]
        config_path: Option<String>,
        /// OpenHands profile name from ~/.openhands/profiles/<name>.json
        #[arg(long)]
        oh_profile: Option<String>,
        /// Override the model name (e.g. "litellm_proxy/cloud-coder").
        /// Takes precedence over profile model and backend defaults.
        #[arg(long)]
        model: Option<String>,
        /// How many times to retry after validation fails (0 = one attempt, no retries)
        #[arg(long, default_value_t = 2)]
        retries: u32,
        /// Push and open draft MR even if validation commands still fail after all retries
        #[arg(long, default_value_t = false)]
        allow_draft_fail: bool,
        /// Load production env file (env_file_prod) instead of dev env_file.
        /// Without this flag, only the dev env_file is loaded.
        #[arg(long, default_value_t = false)]
        prod: bool,
        /// Proceed despite a baseline validation failure the classifier
        /// could not attribute to harness/environment/expected-red.
        #[arg(long, default_value_t = false)]
        allow_unknown_red_baseline: bool,
        /// Seed the initial route decision as a genuine agent-capability
        /// failure, activating cost-aware escalation to a stronger backend
        /// (used by `gah loop --once`'s Escalate action).
        #[arg(long, default_value_t = false)]
        escalate: bool,
        /// TICKET-118: reuse an existing branch for fix operations
        #[arg(long)]
        existing_branch: Option<String>,
    },
    /// Interactive terminal UI: observe state, confirm and run the one
    /// already-decided next action. Does not let you pick an arbitrary
    /// action -- see docs/MANAGER_MEMORY.md "Stretch Goal -- Optional
    /// Operator TUI" (override-next-action is explicitly out of scope).
    Tui {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Manage profiles
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    /// Start the WebSocket server for desktop/web interface
    Server {
        #[arg(long, default_value_t = 3773)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// List all profiles in config
    List {
        #[arg(long)]
        config: Option<String>,
    },
    /// Show details for a profile
    Show {
        name: String,
        #[arg(long)]
        config: Option<String>,
    },
}

#[derive(Subcommand)]
enum LedgerCommands {
    /// Summarize recent ledger entries
    Summary {
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value = "none")]
        group_by: ledger::GroupBy,
    },
    /// Backfill dispatched work with later provider outcomes (MR merged/closed)
    Reconcile {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Availability { json } => availability::cli::run(json)?,

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
        } => doctor::run_with_validate(profile.as_deref(), config_path.as_deref(), validate)?,

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
            } => {
                let cfg = config::load(config_path.as_deref())?;
                ledger::reconcile::run(&cfg, &profile, json)?;
            }
        },

        Commands::Loop {
            profile,
            config_path,
            json,
            once,
        } => {
            if !once {
                anyhow::bail!(
                    "gah loop requires --once; there is no recurring/daemon mode yet -- \
                     see TICKET-079/082"
                );
            }
            let cfg = config::load(config_path.as_deref())?;
            controller::run_once(&cfg, &profile, json)?;
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
            budget,
            dry_run,
            config_path,
            model,
            oh_profile,
            retries,
            allow_draft_fail,
            prod,
            allow_unknown_red_baseline,
            escalate,
            existing_branch,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            dispatch::run(
                &cfg,
                &dispatch::DispatchArgs {
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
                    allow_unknown_red_baseline,
                    escalate,
                    existing_branch,
                },
            )?;
        }

        Commands::Tui {
            profile,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            tui::run(&cfg, profile.as_deref())?;
        }

        Commands::Profile { command } => match command {
            ProfileCommands::List { config } => {
                let cfg = config::load(config.as_deref())?;
                let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
                names.sort_unstable();
                for name in names {
                    let p = &cfg.profiles[name];
                    println!("{:<25} {} ({})", name, p.display_name, p.provider);
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
            }
        },

        Commands::Server { port, host } => {
            println!("Starting WebSocket server on {}:{}", host, port);
            server::run_blocking(&host, port)?;
            // This will run forever, so we don't need to return
            std::thread::park();
        }
    }
    Ok(())
}
