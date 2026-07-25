// Command execution for `gah policy-check` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub config: String,
    pub action: String,
}

pub fn run(args: Args) -> Result<()> {
    crate::policy::run(&args.config, &args.action)
}
