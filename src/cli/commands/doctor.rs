// Command execution for `gah doctor` (ticket #407).

use anyhow::Result;

use crate::doctor;

pub fn run(
    profile: Option<&str>,
    config_path: Option<&str>,
    validate: bool,
    json: bool,
) -> Result<()> {
    doctor::run(profile, config_path, validate, json)
}
