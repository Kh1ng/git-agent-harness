//! Ordered routing-candidate list editing command grammar.
use clap::Subcommand;

#[derive(Subcommand)]
pub enum RoutingCandidateCommands {
    /// Append a candidate to the list.
    Add {
        #[arg(long)]
        profile: String,
        /// Which ordered list: pm | improve | review | escalatory | routine (single reviewer; add replaces it).
        #[arg(long)]
        list: String,
        #[arg(long)]
        backend: String,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        quota_pool: Option<String>,
        #[arg(long, default_value_t = 0)]
        priority: i32,
        #[arg(long, default_value_t = false)]
        included_in_quota: bool,
        #[arg(long)]
        marginal_cost_usd: Option<f64>,
        #[arg(long, default_value_t = false)]
        requires_approval: bool,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        /// Print the resulting order without saving.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Remove the candidate at a 0-based index of the effective list.
    Remove {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        list: String,
        #[arg(long)]
        index: usize,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Move a candidate from one 0-based index to another.
    Move {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        list: String,
        #[arg(long)]
        from: usize,
        #[arg(long)]
        to: usize,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}
