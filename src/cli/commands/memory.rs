//! Explicit, project-scoped memory operations. Automatic capture remains fail-open.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct MemoryArgs {
    #[arg(long, global = true)]
    profile: Option<String>,
    #[arg(long = "config", visible_alias = "config-path", global = true)]
    config_path: Option<String>,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Retrieve relevant project facts.
    Recall { query: String },
    /// List project memory IDs, newest first.
    List {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Back up and delete one project memory ID.
    Delete { id: String },
    /// Flush captured turns into durable memory before starting another chat.
    Flush,
    /// Back up and translate existing project facts, preserving IDs and dates.
    MigrateEnglish,
    /// Read an indexed project scene through the gateway.
    Scene {
        #[arg(long)]
        name: String,
    },
}

pub fn run(args: MemoryArgs) -> Result<()> {
    let profile_name = args
        .profile
        .context("--profile is required for project memory")?;
    let config = crate::config::load(args.config_path.as_deref())?;
    let profile = crate::config::get_profile(&config, &profile_name)?;
    let (operation, payload) = match args.action {
        Action::Recall { query } => {
            anyhow::ensure!(
                !query.trim().is_empty() && query.len() <= 16384,
                "Query must contain 1–16384 bytes"
            );
            ("recall", serde_json::json!({"query":query}))
        }
        Action::List { limit, offset } => {
            anyhow::ensure!((1..=1000).contains(&limit), "Limit must be 1–1000");
            (
                "memories/list",
                serde_json::json!({"limit":limit,"offset":offset}),
            )
        }
        Action::Delete { id } => {
            anyhow::ensure!(
                !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
                "Invalid memory ID"
            );
            ("memories/delete", serde_json::json!({"id":id}))
        }
        Action::Flush => ("session/end", serde_json::json!({})),
        Action::MigrateEnglish => ("memories/migrate-english", serde_json::json!({})),
        Action::Scene { name } => {
            anyhow::ensure!(
                !name.is_empty()
                    && name.len() <= 256
                    && !name.contains(['/', '\\'])
                    && !name.chars().any(char::is_control),
                "Invalid scene name"
            );
            ("profiles/read", serde_json::json!({"filename":name}))
        }
    };
    let result = crate::memory_gateway::project_request(
        &config.defaults,
        &profile_name,
        &profile.local_path,
        operation,
        payload,
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
