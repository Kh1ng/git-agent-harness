// Command execution for `gah tui` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub profile: Option<String>,
    pub config_path: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let cfg = crate::config::load(args.config_path.as_deref())?;
    crate::tui::run(&cfg, args.profile.as_deref())
}
