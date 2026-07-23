// Command execution for `gah prune` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub profile: Option<String>,
    pub config_path: Option<String>,
    pub older_than: Option<u64>,
    pub dry_run: bool,
}

pub fn run(args: Args) -> Result<()> {
    crate::prune::run(
        args.profile.as_deref(),
        args.config_path.as_deref(),
        args.older_than,
        args.dry_run,
    )
}
