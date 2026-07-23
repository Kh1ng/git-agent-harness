// Command execution for `gah candidates` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub gate_artifact: String,
    pub include_warnings: bool,
    pub out_root: String,
}

pub fn run(args: Args) -> Result<()> {
    crate::candidates::run(&args.gate_artifact, args.include_warnings, &args.out_root)
}
