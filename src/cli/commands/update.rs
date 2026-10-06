// Command execution for `gah update` (ticket #410).

use anyhow::Result;

use crate::{update as update_module, update::UpdateArgs};

pub use crate::cli::args::UpdateArgs as Args;

pub fn run(args: Args) -> Result<()> {
    update_module::run(UpdateArgs {
        repo: args.repo,
        pull: args.pull,
        agents: args.agent,
        yes: args.yes,
        role: update_module::HostRole::parse(&args.role)?,
        restart_server: args.restart_server,
        server_service: args.server_service,
        from_release: args.from_release,
        release_manifest: args.release_manifest,
    })
}
