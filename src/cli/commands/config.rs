// Command execution for `gah config` (ticket #407).

use anyhow::Result;

use crate::cli::args::ConfigCommands;
use crate::{config, config_show};

pub fn run(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Show {
            json,
            full,
            profile,
            config_path,
        } => {
            let resolved_config_path = config::resolve_config_path(config_path.as_deref());
            let cfg = config::load(config_path.as_deref())?;
            if json {
                if full {
                    println!(
                        "{}",
                        config_show::config_show_full_json(
                            &cfg,
                            &resolved_config_path,
                            profile.as_deref(),
                        )?
                    );
                } else {
                    // Compatibility contract: bare `config show --json`
                    // remains byte-for-byte the original one-field shape.
                    println!("{}", config_show::config_show_json(&cfg)?);
                }
            } else {
                println!(
                    "current_manager: {}",
                    cfg.defaults.current_manager.as_deref().unwrap_or("(unset)")
                );
            }
        }
        ConfigCommands::Set {
            config_path,
            current_manager,
            clear,
        } => {
            let mut cfg = config::load(config_path.as_deref())?;
            if let Some(v) = current_manager {
                cfg.defaults.current_manager = Some(v);
            } else if clear.contains(&"current_manager".to_string()) {
                cfg.defaults.current_manager = None;
            }
            config::save(&cfg, config_path.as_deref())?;
            println!("Updated global config");
        }
    }
    Ok(())
}
