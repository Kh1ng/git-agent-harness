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
        ConfigCommands::PromptPolicy { command } => {
            return super::prompt_policies::run(command);
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
                // every declared field, not just the flag. This also keeps
                // older readers compatible with field-level inheritance.
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
                if entry.enabled() == enabled {
                    println!(
                        "Backend instance '{}' already {} for profile '{}'",
                        instance, state_word, profile
                    );
                    return Ok(());
                }
                entry.enabled = Some(enabled);
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
        ConfigCommands::AddBackendInstance {
            config_path,
            profile,
            instance,
            runner_kind,
            account_label,
        } => {
            let instance =
                crate::execution_identity::validate_operator_label("backend instance", &instance)?;
            let account_label = crate::execution_identity::validate_operator_label(
                "account label",
                &account_label,
            )?;
            if !matches!(runner_kind.as_str(), "codex" | "claude") {
                anyhow::bail!("runner kind must be 'codex' or 'claude'");
            }
            let mut cfg = config::load(config_path.as_deref())?;
            let state_root = config::default_config_dir()
                .join("backend-instances")
                .join(&instance);
            let mut entry = config::BackendInstanceConfig {
                runner_kind: runner_kind.clone(),
                logical_backend: Some(runner_kind.clone()),
                resolve_from_path: Some(true),
                state_root: Some(state_root.to_string_lossy().into_owned()),
                account_label: Some(account_label),
                auth_source_label: Some(format!("{runner_kind}-cli-login")),
                ..Default::default()
            };
            entry.enabled = Some(matches!(
                crate::runner::resolve_backend_instance_executable(&entry),
                crate::runner::ExecutableResolution::Found(_)
            ));
            {
                let profile_config = cfg
                    .profiles
                    .get_mut(&profile)
                    .ok_or_else(|| anyhow::anyhow!("profile '{}' is not configured", profile))?;
                if profile_config
                    .effective_routing(&cfg.defaults)
                    .backend_instances
                    .contains_key(&instance)
                {
                    anyhow::bail!("backend instance '{}' already exists", instance);
                }
                profile_config
                    .routing
                    .backend_instances
                    .insert(instance.clone(), entry);
                if let Err(errors) =
                    config::check_profile_backend_instances(&cfg.defaults, profile_config)
                {
                    anyhow::bail!(
                        "cannot add backend instance '{}': {}",
                        instance,
                        errors.join("; ")
                    );
                }
            }
            std::fs::create_dir_all(&state_root)?;
            config::save(&cfg, config_path.as_deref())?;
            println!(
                "Added backend instance '{}' for profile '{}'",
                instance, profile
            );
        }
        ConfigCommands::SetBackendInstanceLabel {
            config_path,
            profile,
            instance,
            account_label,
        } => {
            let account_label = crate::execution_identity::validate_operator_label(
                "account label",
                &account_label,
            )?;
            let mut cfg = config::load(config_path.as_deref())?;
            {
                let profile_config = cfg
                    .profiles
                    .get_mut(&profile)
                    .ok_or_else(|| anyhow::anyhow!("profile '{}' is not configured", profile))?;
                let mut entry = profile_config
                    .effective_routing(&cfg.defaults)
                    .backend_instances
                    .get(&instance)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("backend instance '{}' is not declared", instance)
                    })?;
                entry.account_label = Some(account_label);
                profile_config
                    .routing
                    .backend_instances
                    .insert(instance.clone(), entry);
                if let Err(errors) =
                    config::check_profile_backend_instances(&cfg.defaults, profile_config)
                {
                    anyhow::bail!(
                        "cannot update backend instance '{}': {}",
                        instance,
                        errors.join("; ")
                    );
                }
            }
            config::save(&cfg, config_path.as_deref())?;
            println!(
                "Updated backend instance '{}' for profile '{}'",
                instance, profile
            );
        }
        ConfigCommands::ShowBackendInstanceRuntime {
            config_path,
            profile,
            instance,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let profile_config = config::get_profile(&cfg, &profile)?;
            let routing = profile_config.effective_routing(&cfg.defaults);
            let entry = routing.backend_instances.get(&instance).ok_or_else(|| {
                anyhow::anyhow!("backend instance '{}' is not declared", instance)
            })?;
            if !entry.enabled() {
                anyhow::bail!("backend instance '{}' is disabled", instance);
            }
            let executable = match crate::runner::resolve_backend_instance_executable(entry) {
                crate::runner::ExecutableResolution::Found(path) => path,
                _ => anyhow::bail!("backend instance '{}' executable is unavailable", instance),
            };
            println!(
                "{}",
                serde_json::json!({
                    "backend_instance": instance,
                    "runner_kind": entry.runner_kind,
                    "logical_backend": entry.logical_backend.as_deref().unwrap_or(&entry.runner_kind),
                    "executable": executable,
                    "state_root": entry.state_root,
                    "account_label": entry.account_label,
                })
            );
        }
        ConfigCommands::TestBackendInstance {
            config_path,
            profile,
            instance,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let routing = config::get_profile(&cfg, &profile)?.effective_routing(&cfg.defaults);
            let entry = routing.backend_instances.get(&instance).ok_or_else(|| {
                anyhow::anyhow!("backend instance '{}' is not declared", instance)
            })?;
            println!(
                "{}",
                serde_json::json!({
                    "backend_instance": instance,
                    "auth_ready": config::backend_instance_auth_ready(entry),
                })
            );
        }
        ConfigCommands::AuthenticateBackendInstance {
            config_path,
            profile,
            instance,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let profile_config = config::get_profile(&cfg, &profile)?;
            let routing = profile_config.effective_routing(&cfg.defaults);
            let entry = routing.backend_instances.get(&instance).ok_or_else(|| {
                anyhow::anyhow!("backend instance '{}' is not declared", instance)
            })?;
            let executable = match crate::runner::resolve_backend_instance_executable(entry) {
                crate::runner::ExecutableResolution::Found(path) => path,
                _ => anyhow::bail!("backend instance '{}' executable is unavailable", instance),
            };
            let root = entry.state_root.as_deref().ok_or_else(|| {
                anyhow::anyhow!("backend instance '{}' has no isolated state_root", instance)
            })?;
            std::fs::create_dir_all(root)?;
            let mut command = std::process::Command::new(executable);
            command.env("HOME", root);
            match entry.runner_kind.as_str() {
                "codex" => {
                    command
                        .env("CODEX_HOME", std::path::Path::new(root).join(".codex"))
                        .args(["login", "--device-auth"]);
                }
                "claude" => {
                    command
                        .env(
                            "CLAUDE_CONFIG_DIR",
                            std::path::Path::new(root).join(".claude"),
                        )
                        .args(["auth", "login"]);
                }
                other => anyhow::bail!("runner kind '{}' has no managed login flow", other),
            }
            let status = command.status()?;
            if !status.success() {
                anyhow::bail!("provider login failed for backend instance '{}'", instance);
            }
        }
    }
    Ok(())
}
