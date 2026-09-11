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
            node_role,
            registry_central_url,
            clear,
            notification_channel,
            telegram_chat_id,
        } => {
            let mut cfg = if config::resolve_config_path(config_path.as_deref()).exists() {
                config::load(config_path.as_deref())?
            } else {
                config::GahConfig {
                    defaults: Default::default(),
                    profiles: Default::default(),
                    context: Default::default(),
                }
            };
            if let Some(v) = current_manager {
                cfg.defaults.current_manager = Some(v);
            } else if clear.contains(&"current_manager".to_string()) {
                cfg.defaults.current_manager = None;
            }
            if let Some(role) = node_role {
                cfg.defaults.node_role = role;
            }
            if let Some(url) = registry_central_url {
                cfg.defaults.registry_central_url = Some(url);
            }
            if clear.contains(&"registry_central_url".to_string()) {
                cfg.defaults.registry_central_url = None;
            }
            if let Some(raw) = &notification_channel {
                let channel = crate::notify_channels::NotificationChannel::parse(raw)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "unrecognized notification channel '{raw}' (expected none|telegram|discord)"
                        )
                    })?;
                cfg.defaults.notification_channel = channel;
            }
            if let Some(chat_id) = &telegram_chat_id {
                let trimmed = chat_id.trim();
                if trimmed.is_empty() {
                    cfg.defaults.telegram_chat_id = None;
                } else {
                    cfg.defaults.telegram_chat_id = Some(trimmed.to_string());
                }
            }
            crate::node_role::NodeRoleStatus::with_override(&cfg.defaults, None)?;
            config::save(&cfg, config_path.as_deref())?;
            println!("Updated global config");
        }
        ConfigCommands::RoutingCandidate { command } => {
            return super::routing_candidates::run(command);
        }
        ConfigCommands::SetBackendInstanceEnabled {
            config_path,
            profile,
            instance,
            enabled,
        } => {
            let mut cfg = config::load(config_path.as_deref())?;
            let state_word = if enabled { "enabled" } else { "disabled" };
            {
                let profile_config = cfg
                    .profiles
                    .get_mut(&profile)
                    .ok_or_else(|| anyhow::anyhow!("profile '{}' is not configured", profile))?;
                // Resolve the fully merged entry (canonical -> repo defaults ->
                // profile) so the profile-level override written here preserves
                // every declared field, not just the flag (entries replace
                // wholesale by name).
                let merged = profile_config.effective_routing(&cfg.defaults);
                let mut entry = merged
                    .backend_instances
                    .get(&instance)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "backend instance '{}' is not declared for profile '{}'",
                            instance,
                            profile
                        )
                    })?;
                if entry.enabled == enabled {
                    println!(
                        "Backend instance '{}' already {} for profile '{}'",
                        instance, state_word, profile
                    );
                    return Ok(());
                }
                entry.enabled = enabled;
                profile_config
                    .routing
                    .backend_instances
                    .insert(instance.clone(), entry);
                // Validate before saving: the profile must still satisfy the
                // backend-instance contract with the new flag in place.
                if let Err(errors) =
                    config::check_profile_backend_instances(&cfg.defaults, profile_config)
                {
                    anyhow::bail!(
                        "backend instance '{}' cannot be {} for profile '{}': {}",
                        instance,
                        state_word,
                        profile,
                        errors.join("; ")
                    );
                }
            }
            config::save(&cfg, config_path.as_deref())?;
            println!(
                "Backend instance '{}' {} for profile '{}'",
                instance, state_word, profile
            );
        }
    }
    Ok(())
}
