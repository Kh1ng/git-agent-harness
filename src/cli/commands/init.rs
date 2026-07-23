// Command execution for `gah init` (ticket #407).

use anyhow::Result;

use crate::init::{self, InitArgs};

pub fn run(args: InitArgs) -> Result<()> {
    init::run(args)
}
