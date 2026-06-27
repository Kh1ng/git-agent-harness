mod candidates;
mod config;
mod dispatch;
mod models;
mod policy;
mod price_guard;
mod provider;
mod runner;
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
    /// Dispatch a job to a backend (improve, pm, review, fix, experiment)
    Dispatch {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        mode: String,
        #[arg(long, default_value = "auto")]
        backend: String,
        #[arg(long, default_value = "")]
        target: String,
        #[arg(long, default_value = "1")]
        budget: u32,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Path to gah-config.toml (default: auto-discovered)
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Manage profiles
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Candidates {
            gate_artifact,
            include_warnings,
            out_root,
        } => candidates::run(&gate_artifact, include_warnings, &out_root)?,

        Commands::PriceGuard { watchlist, model } => price_guard::run(&watchlist, &model)?,

        Commands::PolicyCheck { config, action } => policy::run(&config, &action)?,

        Commands::Dispatch {
            profile,
            mode,
            backend,
            target,
            budget,
            dry_run,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            dispatch::run(
                &cfg,
                &dispatch::DispatchArgs {
                    profile,
                    mode,
                    backend,
                    target,
                    budget,
                    dry_run,
                    config_path,
                },
            )?;
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
            }
        },
    }
    Ok(())
}
