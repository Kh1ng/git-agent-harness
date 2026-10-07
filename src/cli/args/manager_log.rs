use crate::manager_log::{Diagnosis, Phase};
use clap::{Args, Subcommand};
#[derive(Subcommand)]
pub enum ManagerLogCommands {
    /// Append one manager observation.
    Add(Box<AddArgs>),
    /// Print observations or a per-item summary.
    Show {
        #[arg(long)]
        work_id: Option<String>,
        #[arg(long)]
        summary: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        config_path: Option<String>,
    },
}
#[derive(Args)]
pub struct AddArgs {
    #[arg(long)]
    pub work_id: String,
    #[arg(long, value_enum)]
    pub phase: Phase,
    #[arg(long)]
    pub tier: Option<u8>,
    #[arg(long)]
    pub attempt: Option<u64>,
    #[arg(long)]
    pub backend: Option<String>,
    #[arg(long, value_enum)]
    pub diagnosis: Option<Diagnosis>,
    #[arg(long)]
    pub intervention: Option<String>,
    #[arg(long)]
    pub tokens: Option<u64>,
    #[arg(long)]
    pub elapsed_seconds: Option<u64>,
    #[arg(long)]
    pub manager_rounds: Option<u64>,
    #[arg(long)]
    pub outcome: Option<String>,
    #[arg(long)]
    pub note: Option<String>,
    #[arg(long)]
    pub owner: Option<String>,
    #[arg(long)]
    pub config_path: Option<String>,
}
