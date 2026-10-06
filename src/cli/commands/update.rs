// Command execution for `gah update` (ticket #410).

use anyhow::Result;

use std::path::PathBuf;

use crate::{update as update_module, update::UpdateArgs};

pub struct Args {
    pub repo: Option<PathBuf>,
    pub pull: bool,
    pub agent: Vec<String>,
    pub yes: bool,
    pub role: String,
    pub restart_server: bool,
    pub server_service: String,
}

pub fn run(args: Args) -> Result<()> {
    update_module::run(UpdateArgs {
        repo: args.repo,
        pull: args.pull,
        agents: args.agent,
        yes: args.yes,
        role: update_module::HostRole::parse(&args.role)?,
        restart_server: args.restart_server,
        server_service: args.server_service,
    })
}
