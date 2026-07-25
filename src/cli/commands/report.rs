// Command execution for `gah report` (ticket #409).

use anyhow::Result;

pub struct Args {
    pub since: String,
    pub profile: Option<String>,
    pub config_path: Option<String>,
    pub group_by: crate::ledger::GroupBy,
    pub json: bool,
    pub series: bool,
    pub bucket: String,
}

pub fn run(args: Args) -> Result<()> {
    crate::report::run(crate::report::ReportArgs {
        since: args.since,
        profile: args.profile,
        config_path: args.config_path,
        group_by: args.group_by,
        json: args.json,
        series: args.series,
        bucket: args.bucket,
    })?;
    Ok(())
}
