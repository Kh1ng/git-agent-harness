// Command execution for `gah node` (issue #944).

use anyhow::Result;

use crate::cli::args::NodeCommands;
use crate::node_register::{self, RegisterOptions};

pub fn run(command: NodeCommands) -> Result<()> {
    match command {
        NodeCommands::Register {
            config_path,
            central_url,
            advertised_url,
            transport_mode,
            secret_ref,
            profiles,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let central = central_url
                .clone()
                .or_else(|| cfg.defaults.registry_central_url.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no central URL: pass --central-url or set [defaults] registry_central_url"
                    )
                })?;
            let profile_list = profiles
                .map(|p| {
                    p.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .filter(|l| !l.is_empty());

            node_register::register(&RegisterOptions {
                central_url: central.clone(),
                advertised_url,
                transport_mode,
                secret_ref,
                profiles: profile_list,
                token: std::env::var("COORDINATOR_TOKEN").ok(),
                config_path,
                identity_path: None,
            })?;
            println!("Registered node against {central}");
            Ok(())
        }
    }
}
