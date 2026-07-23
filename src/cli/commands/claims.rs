// Command execution for `gah claims` (ticket #409).

use anyhow::Result;

use crate::cli::args::ClaimsCommands;
use crate::{work_claim, work_claim::canonical_scope_for_profile};

pub fn run(command: ClaimsCommands) -> Result<()> {
    match command {
        ClaimsCommands::List {
            json,
            profile,
            config_path,
        } => {
            let scope = profile
                .map(|profile: String| {
                    canonical_scope_for_profile(&profile, config_path.as_deref())
                })
                .transpose()?;
            work_claim::handle_claims_list(scope.as_deref(), json)?;
        }
        ClaimsCommands::Clear {
            work_id,
            profile,
            config_path,
        } => {
            let scope = canonical_scope_for_profile(&profile, config_path.as_deref())?;
            work_claim::handle_claims_clear(&scope, &work_id)?;
            println!("Cleared claim for work_id {work_id} on profile {profile}");
        }
        ClaimsCommands::Reclaim {
            profile,
            max_age_secs,
        } => {
            let scope = canonical_scope_for_profile(&profile, None)?;
            let reclaimed = work_claim::handle_claims_reclaim(&scope, max_age_secs)?;
            if reclaimed.is_empty() {
                println!("No stale claims to reclaim for profile {}", profile);
            } else {
                println!(
                    "Reclaimed {} stale claim(s) for profile {}: {}",
                    reclaimed.len(),
                    profile,
                    reclaimed.join(", ")
                );
            }
        }
    }
    Ok(())
}
