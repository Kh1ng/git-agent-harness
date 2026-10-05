use crate::cli::args::CredentialCommands;
use crate::credentials::{self, CredentialInfo, CredentialKind};
use anyhow::{bail, Result};
use std::io::Read;

pub fn run(command: CredentialCommands) -> Result<()> {
    match command {
        CredentialCommands::List { json } => {
            let records = credentials::list()?;
            if json {
                println!("{}", serde_json::to_string(&records)?);
            } else {
                for record in records {
                    let kind = match record.kind {
                        CredentialKind::ApiKey => "api_key",
                        CredentialKind::ClaudeSubscriptionToken => "claude_subscription_token",
                        CredentialKind::MistralDashboard => "mistral_dashboard",
                        CredentialKind::MistralLogin => "mistral_login",
                    };
                    println!("{}\t{}\t{}", record.id, record.provider, kind);
                }
            }
        }
        CredentialCommands::Save {
            id,
            provider,
            kind,
            account_label,
            env_var,
            json,
        } => {
            let mut secret = String::new();
            std::io::stdin()
                .take(32769)
                .read_to_string(&mut secret)
                .map_err(|_| anyhow::anyhow!("cannot read credential input"))?;
            if secret.len() > 32768 {
                bail!("credential input exceeds size limit");
            }
            let info = credentials::save(
                CredentialInfo {
                    id,
                    provider,
                    kind,
                    account_label,
                    env_var,
                },
                secret.trim_end_matches(['\r', '\n']),
            )?;
            if json {
                println!("{}", serde_json::to_string(&info)?);
            } else {
                println!("Saved named credential.");
            }
        }
        CredentialCommands::Remove { id } => {
            credentials::remove(&id)?;
            println!("Removed named credential.");
        }
    }
    Ok(())
}
