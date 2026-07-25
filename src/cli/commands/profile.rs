// Command execution for `gah profile` (ticket #407).

use anyhow::Result;

use crate::cli::args::{parse_delivery_mode, parse_wake_autonomy, ProfileCommands};
use crate::config::Profile;
use crate::{config, profile_output};

pub fn run(command: ProfileCommands) -> Result<()> {
    match command {
        ProfileCommands::List { config, json } => {
            let cfg = config::load(config.as_deref())?;
            let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
            names.sort_unstable();
            if json {
                println!("{}", profile_output::list_json(&cfg)?);
            } else {
                for name in names {
                    let p = &cfg.profiles[name];
                    println!("{:<25} {} ({})", name, p.display_name, p.provider);
                }
            }
        }
        ProfileCommands::Show { name, config } => {
            let cfg = config::load(config.as_deref())?;
            let p = config::get_profile(&cfg, &name)?;
            println!("name:                  {}", name);
            println!("display_name:          {}", p.display_name);
            println!("repo_id:               {}", p.repo_id);
            println!("provider:              {}", p.provider);
            println!("repo:                  {}", p.repo);
            println!("local_path:            {}", p.local_path);
            println!("artifact_root:         {}", p.artifact_root);
            println!("default_target_branch: {}", p.default_target_branch);
            if let Some(api) = &p.provider_api_base {
                println!("provider_api_base:     {}", api);
            }
            if let Some(id) = &p.provider_project_id {
                println!("provider_project_id:   {}", id);
            }
            if !p.openhands_args.is_empty() {
                println!("openhands_args:        {:?}", p.openhands_args);
            }
            if !p.codex_args.is_empty() {
                println!("codex_args:            {:?}", p.codex_args);
            }
            if !p.claude_args.is_empty() {
                println!("claude_args:           {:?}", p.claude_args);
            }
            if !p.validation_commands.is_empty() {
                println!("validation_commands:");
                for cmd in &p.validation_commands {
                    println!("  - {}", cmd);
                }
            }
            println!(
                "validation_timeout_seconds: {}",
                p.validation_timeout_seconds()
            );
        }
        ProfileCommands::Add {
            name,
            display_name,
            repo_id,
            provider,
            repo,
            local_path,
            artifact_root,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            config_path,
            openhands_args,
            codex_args,
            codex_path,
            claude_args,
            claude_path,
            agy_path,
            vibe_args,
            vibe_path,
            opencode_args,
            opencode_path,
            agy_second_home,
            notify_command,
            policy_path,
            env_file,
            env_file_prod,
            validation_commands,
            auto_fix_commands,
            max_parallel_workers,
            max_open_managed_mrs,
            validation_timeout_seconds,
            manager_wake_autonomy,
            delivery_mode,
        } => {
            let mut cfg = config::load(config_path.as_deref())?;
            let profile = Profile {
                display_name,
                repo_id,
                provider,
                repo,
                local_path,
                artifact_root,
                default_target_branch,
                provider_api_base,
                provider_project_id,
                oh_profile: None,
                openhands_args,
                codex_args,
                codex_path,
                claude_args,
                claude_path,
                agy_path,
                vibe_args,
                vibe_path,
                opencode_args,
                opencode_path,
                agy_second_home,
                agy_print_timeout_seconds: std::collections::HashMap::new(),
                agy_idle_timeout_seconds: None,
                opencode_idle_timeout_seconds: None,
                opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
                max_concurrent_per_model: std::collections::HashMap::new(),
                openhands_idle_timeout_seconds: None,
                vibe_idle_timeout_seconds: None,
                codex_idle_timeout_seconds: None,
                claude_idle_timeout_seconds: None,
                max_parallel_workers,
                max_open_managed_mrs,
                notify_command,
                manager_wake_autonomy: match &manager_wake_autonomy {
                    Some(v) => parse_wake_autonomy(v)?,
                    None => config::WakeAutonomy::default(),
                },
                delivery_mode: match &delivery_mode {
                    Some(v) => parse_delivery_mode(v)?,
                    None => config::DeliveryMode::default(),
                },
                policy_path,
                env_file,
                env_file_prod,
                validation_commands,
                auto_fix_commands,
                test_file_patterns: vec![],
                known_baseline_failure_markers: vec![],
                model_improve: None,
                model_pm: None,
                model_review: None,
                review_timeout_seconds: None,
                review_hard_timeout_seconds: None,
                validation_timeout_seconds,
                routing: config::RoutingPolicy::default(),
                publishing: Default::default(),
                pacing: Default::default(),
                prune_older_than_days: None,
            };
            config::add_profile(&mut cfg, &name, profile)?;
            config::save(&cfg, config_path.as_deref())?;
            println!("Added profile '{}'", name);
        }
        ProfileCommands::Set {
            name,
            display_name,
            repo_id,
            provider,
            repo,
            local_path,
            artifact_root,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            config_path,
            openhands_args,
            codex_args,
            codex_path,
            claude_args,
            claude_path,
            agy_path,
            vibe_args,
            vibe_path,
            opencode_args,
            opencode_path,
            agy_second_home,
            notify_command,
            policy_path,
            env_file,
            env_file_prod,
            validation_commands,
            auto_fix_commands,
            max_parallel_workers,
            max_open_managed_mrs,
            validation_timeout_seconds,
            manager_wake_autonomy,
            delivery_mode,
            clear,
        } => {
            let mut cfg = config::load(config_path.as_deref())?;
            let existing = config::get_profile_mut(&mut cfg, &name)?;

            // Helper to clear a field if requested
            fn should_clear(field: &str, clear_list: &[String]) -> bool {
                clear_list.contains(&field.to_string())
            }

            // Update fields if provided, or clear if requested
            if let Some(v) = display_name {
                existing.display_name = v;
            } else if should_clear("display_name", &clear) {
                // display_name is required, so we don't actually clear it
            }

            if let Some(v) = repo_id {
                existing.repo_id = v;
            } else if should_clear("repo_id", &clear) {
                // repo_id is required
            }

            if let Some(v) = provider {
                existing.provider = v;
            } else if should_clear("provider", &clear) {
                // provider is required
            }

            if let Some(v) = repo {
                existing.repo = v;
            } else if should_clear("repo", &clear) {
                // repo is required
            }

            if let Some(v) = local_path {
                existing.local_path = v;
            } else if should_clear("local_path", &clear) {
                // local_path is required
            }

            if let Some(v) = artifact_root {
                existing.artifact_root = v;
            } else if should_clear("artifact_root", &clear) {
                // artifact_root is required
            }

            if let Some(v) = default_target_branch {
                existing.default_target_branch = v;
            } else if should_clear("default_target_branch", &clear) {
                // default_target_branch is required
            }

            if let Some(v) = provider_api_base {
                existing.provider_api_base = Some(v);
            } else if should_clear("provider_api_base", &clear) {
                existing.provider_api_base = None;
            }

            if let Some(v) = provider_project_id {
                existing.provider_project_id = Some(v);
            } else if should_clear("provider_project_id", &clear) {
                existing.provider_project_id = None;
            }

            if !openhands_args.is_empty() {
                existing.openhands_args = openhands_args;
            } else if should_clear("openhands_args", &clear) {
                existing.openhands_args.clear();
            }

            if !codex_args.is_empty() {
                existing.codex_args = codex_args;
            } else if should_clear("codex_args", &clear) {
                existing.codex_args.clear();
            }

            if let Some(v) = codex_path {
                existing.codex_path = Some(v);
            } else if should_clear("codex_path", &clear) {
                existing.codex_path = None;
            }

            if !claude_args.is_empty() {
                existing.claude_args = claude_args;
            } else if should_clear("claude_args", &clear) {
                existing.claude_args.clear();
            }

            if let Some(v) = claude_path {
                existing.claude_path = Some(v);
            } else if should_clear("claude_path", &clear) {
                existing.claude_path = None;
            }

            if let Some(v) = agy_path {
                existing.agy_path = Some(v);
            } else if should_clear("agy_path", &clear) {
                existing.agy_path = None;
            }

            if !vibe_args.is_empty() {
                existing.vibe_args = vibe_args;
            } else if should_clear("vibe_args", &clear) {
                existing.vibe_args.clear();
            }

            if let Some(v) = vibe_path {
                existing.vibe_path = Some(v);
            } else if should_clear("vibe_path", &clear) {
                existing.vibe_path = None;
            }

            if !opencode_args.is_empty() {
                existing.opencode_args = opencode_args;
            } else if should_clear("opencode_args", &clear) {
                existing.opencode_args.clear();
            }

            if let Some(v) = opencode_path {
                existing.opencode_path = Some(v);
            } else if should_clear("opencode_path", &clear) {
                existing.opencode_path = None;
            }

            if let Some(v) = agy_second_home {
                existing.agy_second_home = Some(v);
            } else if should_clear("agy_second_home", &clear) {
                existing.agy_second_home = None;
            }

            if let Some(v) = notify_command {
                existing.notify_command = Some(v);
            } else if should_clear("notify_command", &clear) {
                existing.notify_command = None;
            }

            if let Some(v) = policy_path {
                existing.policy_path = Some(v);
            } else if should_clear("policy_path", &clear) {
                existing.policy_path = None;
            }

            if let Some(v) = env_file {
                existing.env_file = Some(v);
            } else if should_clear("env_file", &clear) {
                existing.env_file = None;
            }

            if let Some(v) = env_file_prod {
                existing.env_file_prod = Some(v);
            } else if should_clear("env_file_prod", &clear) {
                existing.env_file_prod = None;
            }

            if !validation_commands.is_empty() {
                existing.validation_commands = validation_commands;
            } else if should_clear("validation_commands", &clear) {
                existing.validation_commands.clear();
            }

            if !auto_fix_commands.is_empty() {
                existing.auto_fix_commands = auto_fix_commands;
            } else if should_clear("auto_fix_commands", &clear) {
                existing.auto_fix_commands.clear();
            }

            if let Some(v) = max_parallel_workers {
                existing.max_parallel_workers = Some(v);
            } else if should_clear("max_parallel_workers", &clear) {
                existing.max_parallel_workers = None;
            }

            if let Some(v) = max_open_managed_mrs {
                existing.max_open_managed_mrs = Some(v);
            } else if should_clear("max_open_managed_mrs", &clear) {
                existing.max_open_managed_mrs = None;
            }

            if let Some(v) = validation_timeout_seconds {
                existing.validation_timeout_seconds = Some(v);
            } else if should_clear("validation_timeout_seconds", &clear) {
                existing.validation_timeout_seconds = None;
            }

            if let Some(v) = &manager_wake_autonomy {
                existing.manager_wake_autonomy = parse_wake_autonomy(v)?;
            } else if should_clear("manager_wake_autonomy", &clear) {
                existing.manager_wake_autonomy = config::WakeAutonomy::default();
            }

            if let Some(v) = &delivery_mode {
                existing.delivery_mode = parse_delivery_mode(v)?;
            } else if should_clear("delivery_mode", &clear) {
                existing.delivery_mode = config::DeliveryMode::default();
            }

            config::save(&cfg, config_path.as_deref())?;
            println!("Updated profile '{}'", name);
        }
        ProfileCommands::Remove {
            name,
            config_path,
            force,
        } => {
            if !force {
                eprintln!("Warning: Removing profile '{}' cannot be undone.", name);
                eprintln!("Use --force to confirm.");
                anyhow::bail!("Aborted");
            }
            let mut cfg = config::load(config_path.as_deref())?;
            config::remove_profile(&mut cfg, &name)?;
            config::save(&cfg, config_path.as_deref())?;
            println!("Removed profile '{}'", name);
        }
    }
    Ok(())
}
