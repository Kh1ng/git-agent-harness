// Command execution for `gah watchdog-check` (issue #726).

use anyhow::Result;

pub struct Args {
    pub profile: Option<String>,
    pub config_path: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    crate::watchdog::run(args.profile.as_deref(), args.config_path.as_deref())
}
