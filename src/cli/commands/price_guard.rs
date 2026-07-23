// Command execution for `gah price-guard` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub watchlist: String,
    pub model: String,
}

pub fn run(args: Args) -> Result<()> {
    crate::price_guard::run(&args.watchlist, &args.model)
}
