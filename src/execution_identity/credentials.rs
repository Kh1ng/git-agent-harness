use super::ExecutionIdentity;
use std::path::PathBuf;

impl ExecutionIdentity {
    /// Resolve a named source only for this attempt. No secret is added to
    /// identity, config projections, or the parent process environment.
    pub fn credential_env(&self) -> anyhow::Result<Vec<(String, String)>> {
        let Some(id) = self.credential_id.as_deref() else {
            return Ok(Vec::new());
        };
        if self.state_root.is_none() {
            anyhow::bail!("named credentials require an isolated instance state_root");
        }
        let source = match crate::credentials::get(id) {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential unavailable"),
        };
        let provider = match self.runner_kind.as_str() {
            "claude" => "anthropic",
            "codex" => "openai",
            "vibe" => "mistral",
            "agy" => anyhow::bail!("AGY instances use isolated native login, not API-key bindings"),
            "opencode" => self
                .effective_model
                .as_deref()
                .and_then(|model| model.split_once('/'))
                .map(|(provider, _)| provider)
                .unwrap_or(&source.provider),
            "hermes" | "openhands" => &source.provider,
            _ => anyhow::bail!("runner does not support credential bindings"),
        };
        if source
            .env_var
            .as_deref()
            .is_some_and(|name| name.contains("_ADMIN_") || name.contains("_ADMINISTRATOR_"))
        {
            anyhow::bail!("administrative credentials cannot run inference");
        }
        let subscription = source.kind == crate::credentials::CredentialKind::ClaudeSubscription;
        let mut env = match crate::credentials::execution_env(
            id,
            if subscription {
                &self.runner_kind
            } else {
                provider
            },
        ) {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential does not match the execution provider"),
        };
        // Keep the provider's standard scope name in the approval projection,
        // even when the owner selected an alternate environment variable.
        let standard = match crate::credentials::canonical_provider(&source.provider) {
            _ if subscription => None,
            "openai" => Some("OPENAI_API_KEY"),
            "anthropic" => Some("ANTHROPIC_API_KEY"),
            "mistral" => Some("MISTRAL_API_KEY"),
            "nous" => Some("NOUS_API_KEY"),
            "google" => Some("GOOGLE_API_KEY"),
            "openrouter" => Some("OPENROUTER_API_KEY"),
            "xai" => Some("XAI_API_KEY"),
            "kimi" => Some("MOONSHOT_API_KEY"),
            _ => None,
        };
        if let Some(standard) = standard {
            if env[0].0 != standard {
                env.push((standard.into(), env[0].1.clone()));
            }
        }
        let required = match self.runner_kind.as_str() {
            "claude" if !subscription => Some("ANTHROPIC_API_KEY"),
            "vibe" => Some("MISTRAL_API_KEY"),
            "openhands" => Some("LLM_API_KEY"),
            _ => None,
        };
        if let Some(required) = required {
            let key = env
                .first()
                .ok_or_else(|| anyhow::anyhow!("credential has no execution key"))?
                .1
                .clone();
            if !env.iter().any(|(name, _)| name == required) {
                env.push((required.into(), key));
            }
        }
        Ok(env)
    }

    /// Account state is isolated for every native execution path.
    pub fn apply_instance_state_env(&self, env: &mut Vec<(String, String)>) {
        let Some(root) = self.state_root.as_ref() else {
            return;
        };
        let mut values = vec![("HOME", root.clone())];
        match self.runner_kind.as_str() {
            "codex" => values.push(("CODEX_HOME", root.join(".codex"))),
            "claude" => values.push(("CLAUDE_CONFIG_DIR", root.join(".claude"))),
            "hermes" => values.push(("HERMES_HOME", root.join(".hermes"))),
            "vibe" => values.push(("VIBE_HOME", root.join(".vibe"))),
            "opencode" => values.extend([
                ("XDG_CONFIG_HOME", root.join(".config")),
                ("XDG_DATA_HOME", root.join(".local/share")),
                ("XDG_STATE_HOME", root.join(".local/state")),
                ("XDG_CACHE_HOME", root.join(".cache")),
            ]),
            _ => {}
        }
        for (name, value) in values {
            env.retain(|(key, _)| key != name);
            env.push((name.into(), value.to_string_lossy().into_owned()));
        }
    }

    /// Apply the selected source after ambient/profile values. OpenCode's
    /// explicit provider override also defeats literal keys in its config.
    pub fn apply_credential_env(&self, env: &mut Vec<(String, String)>) -> anyhow::Result<()> {
        let selected = match self.credential_env() {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential unavailable"),
        };
        for (key, value) in &selected {
            env.retain(|(name, _)| name != key);
            env.push((key.clone(), value.clone()));
        }
        if self.runner_kind == "vibe" && !selected.is_empty() {
            env.retain(|(name, _)| name != "VIBE_CLI" && name != "GAH_VIBE_CREDENTIAL_BOUND");
            env.push(("GAH_VIBE_CREDENTIAL_BOUND".into(), "1".into()));
            if let Some(model) = &self.effective_model {
                env.retain(|(name, _)| name != "VIBE_ACTIVE_MODEL");
                env.push(("VIBE_ACTIVE_MODEL".into(), model.clone()));
            }
        }
        if self.runner_kind == "claude" && !selected.is_empty() {
            filter_claude_selected_env(env, &selected);
        }
        if self.runner_kind == "codex" && !selected.is_empty() {
            let key_var = &selected[0].0;
            let config = serde_json::json!({
                "model_provider": "gah_selected_openai",
                "model_providers": {"gah_selected_openai": {
                    "name": "OpenAI", "base_url": "https://api.openai.com/v1", "wire_api": "responses",
                    "env_key": key_var, "requires_openai_auth": false
                }}
            });
            env.retain(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "CODEX_CONFIG" | "MODEL_PROVIDER" | "DEFAULT_AUTH_REQUEST"
                ) && (!matches!(name.as_str(), "CODEX_API_KEY" | "OPENAI_API_KEY")
                    || name == key_var)
            });
            env.push(("CODEX_CONFIG".into(), config.to_string()));
            env.push(("MODEL_PROVIDER".into(), "gah_selected_openai".into()));
        }
        if self.runner_kind == "openhands" && !selected.is_empty() {
            let source =
                crate::credentials::get(self.credential_id.as_deref().expect("selected source"))?;
            let endpoint = provider_endpoint(&source.provider).ok_or_else(|| {
                anyhow::anyhow!(
                    "this OpenHands credential provider needs an explicit supported endpoint"
                )
            })?;
            env.retain(|(name, _)| name != "LLM_BASE_URL");
            env.push(("LLM_BASE_URL".into(), endpoint.into()));
            if let Some(model) = &self.effective_model {
                env.retain(|(name, _)| name != "LLM_MODEL");
                env.push(("LLM_MODEL".into(), model.clone()));
            }
        }
        if self.runner_kind == "opencode" && !selected.is_empty() {
            let source =
                crate::credentials::get(self.credential_id.as_deref().expect("selected source"))?;
            let provider = self
                .effective_model
                .as_deref()
                .and_then(|model| model.split_once('/'))
                .map(|(provider, _)| provider)
                .unwrap_or(&source.provider);
            let inherited = env
                .iter()
                .rev()
                .find(|(name, _)| name == "OPENCODE_CONFIG_CONTENT")
                .map(|(_, value)| value.clone())
                .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
            let mut config = match inherited {
                Some(value) => serde_json::from_str::<serde_json::Value>(&value)
                    .map_err(|_| anyhow::anyhow!("OpenCode child configuration is invalid"))?,
                None => serde_json::json!({}),
            };
            let root = config
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("OpenCode child configuration must be an object"))?;
            let providers = root
                .entry("provider")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    anyhow::anyhow!("OpenCode provider configuration must be an object")
                })?;
            let configured = providers
                .entry(provider)
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("OpenCode selected provider must be an object"))?;
            let options = configured
                .entry("options")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("OpenCode provider options must be an object"))?;
            if let Some(endpoint) = provider_endpoint(&source.provider) {
                options.insert("baseURL".into(), serde_json::Value::String(endpoint.into()));
            }
            options.insert(
                "apiKey".into(),
                serde_json::Value::String(format!("{{env:{}}}", selected[0].0)),
            );
            if let Some(model) = &self.effective_model {
                root.insert("model".into(), serde_json::Value::String(model.clone()));
            }
            env.retain(|(name, _)| name != "OPENCODE_CONFIG_CONTENT");
            env.push(("OPENCODE_CONFIG_CONTENT".into(), config.to_string()));
        }
        Ok(())
    }

    /// GAH provisions only an unused isolated Hermes home, with an env reference.
    /// Existing provider configuration is never overwritten or converted.
    pub fn provision_credential_config(&self) -> anyhow::Result<()> {
        if self.runner_kind != "hermes" || self.credential_id.is_none() {
            return Ok(());
        }
        let expected = self.hermes_credential_config()?;
        let root = self
            .state_root
            .as_ref()
            .expect("validated isolated state")
            .join(".hermes");
        std::fs::create_dir_all(&root)?;
        let path = root.join("config.yaml");
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(expected.to_string().as_bytes())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => anyhow::bail!("cannot provision isolated Hermes provider configuration"),
        }
        let selected = match self.credential_env() {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential unavailable"),
        };
        let reference = format!("{}=${{{}}}\n", selected[0].0, selected[0].0);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(".env"))
        {
            Ok(mut file) => file.write_all(reference.as_bytes())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.validate_hermes_config()?
            }
            Err(_) => anyhow::bail!("cannot provision isolated Hermes credential reference"),
        }
        self.validate_hermes_config()
    }

    fn hermes_credential_config(&self) -> anyhow::Result<serde_json::Value> {
        let selected = match self.credential_env() {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential unavailable"),
        };
        let source = crate::credentials::get(self.credential_id.as_deref().expect("named source"))?;
        let endpoint = provider_endpoint(&source.provider).ok_or_else(|| {
            anyhow::anyhow!(
                "this Hermes provider needs a supported isolated env-reference configuration"
            )
        })?;
        Ok(
            serde_json::json!({"model": {"provider": "custom:gah-selected"}, "custom_providers": [{"name": "gah-selected", "base_url": endpoint, "key_env": selected[0].0}]}),
        )
    }

    fn validate_hermes_config(&self) -> anyhow::Result<()> {
        let expected = self.hermes_credential_config()?;
        let root = self.state_root.as_ref().expect("validated isolated state");
        let home = root.join(".hermes");
        let selected = match self.credential_env() {
            Ok(value) => value,
            Err(_) => anyhow::bail!("named credential unavailable"),
        };
        let reference = format!("{}=${{{}}}\n", selected[0].0, selected[0].0);
        if std::fs::read_to_string(home.join(".env")).ok().as_deref() != Some(&reference)
            || home.join("auth.json").try_exists().unwrap_or(true)
            || home.join(".op.env").try_exists().unwrap_or(true)
        {
            anyhow::bail!("Hermes named credentials require an isolated env reference without native credential pools");
        }
        let text = std::fs::read_to_string(root.join(".hermes/config.yaml")).map_err(|_| {
            anyhow::anyhow!("configure the isolated Hermes named credential before execution")
        })?;
        let actual: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| anyhow::anyhow!("Hermes named credentials require GAH's isolated env-reference provider configuration"))?;
        if actual.get("model").and_then(|model| model.get("provider"))
            != expected.pointer("/model/provider")
            || actual.get("custom_providers") != expected.get("custom_providers")
            || actual.get("providers").is_some()
            || actual.get("model_aliases").is_some()
            || actual.get("fallback_models").is_some()
            || actual.get("env").is_some()
            || actual.pointer("/model/base_url").is_some()
            || actual.pointer("/model/api_key").is_some()
        {
            anyhow::bail!("Hermes configuration overrides the selected credential; use its isolated env-reference provider configuration");
        }
        Ok(())
    }

    /// CLI overrides must not replace a preflighted provider configuration.
    pub fn validate_credential_args(&self, args: &[String]) -> anyhow::Result<()> {
        if self.credential_id.is_none() {
            return Ok(());
        }
        if self.runner_kind == "opencode"
            && self.effective_model.is_none()
            && !args.iter().any(|arg| arg == "acp")
        {
            anyhow::bail!(
                "a named OpenCode credential requires an explicit provider-qualified model"
            );
        }
        let forbidden: &[&str] = match self.runner_kind.as_str() {
            "claude" => &["--settings", "--setting-sources"],
            "hermes" => &["--provider", "--base-url", "--api-key"],
            "opencode" => &["--model", "-m"],
            _ => &[],
        };
        if args.iter().any(|arg| {
            forbidden
                .iter()
                .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
        }) {
            anyhow::bail!("runner arguments override this named credential configuration");
        }
        Ok(())
    }

    /// Refuse Claude settings that can replace a selected API source. The
    /// project and managed settings retain their existing authority.
    pub fn validate_launch_config(&self, cwd: &std::path::Path) -> anyhow::Result<()> {
        if self.credential_id.is_none() {
            return Ok(());
        }
        if self.runner_kind == "vibe" {
            return self.validate_vibe_config(cwd);
        }
        if self.runner_kind == "hermes" {
            return self.validate_hermes_config();
        }
        if self.runner_kind == "codex" {
            let root = self
                .state_root
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("named API credentials require isolated state"))?;
            for path in [
                root.join(".codex/config.toml"),
                cwd.join(".codex/config.toml"),
            ] {
                let text = match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => anyhow::bail!("cannot validate Codex credential configuration"),
                };
                let config: toml::Value = toml::from_str(&text).map_err(|_| {
                    anyhow::anyhow!("cannot validate Codex credential configuration")
                })?;
                if config
                    .get("model_providers")
                    .and_then(|providers| providers.get("gah_selected_openai"))
                    .and_then(toml::Value::as_table)
                    .is_some_and(|provider| {
                        provider.keys().any(|name| {
                            matches!(
                                name.as_str(),
                                "http_headers"
                                    | "env_http_headers"
                                    | "experimental_bearer_token"
                                    | "experimental_bearer_token_env_var"
                            )
                        })
                    })
                {
                    anyhow::bail!("Codex provider auth headers override this named credential; remove competing isolated provider settings");
                }
            }
            return Ok(());
        }
        if self.runner_kind != "claude" {
            return Ok(());
        }
        let root = self
            .state_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("named API credentials require isolated state"))?;
        let mut paths = vec![
            root.join(".claude/settings.json"),
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
        ];
        if cfg!(target_os = "macos") {
            paths.push(PathBuf::from(
                "/Library/Application Support/ClaudeCode/managed-settings.json",
            ));
        }
        if cfg!(target_os = "linux") {
            paths.push(PathBuf::from("/etc/claude-code/managed-settings.json"));
        }
        if cfg!(target_os = "windows") {
            paths.push(PathBuf::from(
                "C:/Program Files/ClaudeCode/managed-settings.json",
            ));
        }
        for path in paths {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => anyhow::bail!("cannot validate Claude credential settings"),
            };
            let settings: serde_json::Value = serde_json::from_str(&text)
                .map_err(|_| anyhow::anyhow!("cannot validate Claude credential settings"))?;
            if settings
                .get("apiKeyHelper")
                .is_some_and(|value| value.as_str() != Some(""))
                || settings.get("forceLoginMethod").is_some()
                || settings
                    .get("env")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|env| {
                        env.keys().any(|name| {
                            name.starts_with("ANTHROPIC_")
                                || name.starts_with("CLAUDE_CODE_OAUTH")
                                || name.starts_with("CLAUDE_CODE_USE_")
                        })
                    })
            {
                anyhow::bail!("Claude settings override this named credential; use an isolated API configuration without competing auth settings");
            }
        }
        Ok(())
    }
}

fn filter_claude_selected_env(env: &mut Vec<(String, String)>, selected: &[(String, String)]) {
    env.retain(|(name, _)| {
        (!name.starts_with("ANTHROPIC_") || selected.iter().any(|(key, _)| key == name))
            && (!name.starts_with("CLAUDE_CODE_OAUTH")
                || selected.iter().any(|(key, _)| key == name))
            && !matches!(
                name.as_str(),
                "CLAUDE_CODE_USE_BEDROCK" | "CLAUDE_CODE_USE_VERTEX" | "CLAUDE_CODE_USE_FOUNDRY"
            )
    });
}

/// Native Codex uses TOML CLI overrides; its ACP bridge uses CODEX_CONFIG.
/// Both refer to the same selected environment variable, never the key itself.
pub fn selected_codex_config_args(env: &[(String, String)]) -> Vec<String> {
    let Some(raw) = env
        .iter()
        .rev()
        .find(|(name, _)| name == "CODEX_CONFIG")
        .map(|(_, value)| value)
    else {
        return vec![];
    };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(raw) else {
        return vec![];
    };
    if config
        .get("model_provider")
        .and_then(serde_json::Value::as_str)
        != Some("gah_selected_openai")
    {
        return vec![];
    }
    let Some(provider) = config
        .pointer("/model_providers/gah_selected_openai")
        .and_then(serde_json::Value::as_object)
    else {
        return vec![];
    };
    let mut args = vec!["-c".into(), "model_provider=\"gah_selected_openai\"".into()];
    for field in [
        "name",
        "base_url",
        "wire_api",
        "env_key",
        "requires_openai_auth",
    ] {
        if let Some(value) = provider.get(field) {
            args.extend([
                "-c".into(),
                format!("model_providers.gah_selected_openai.{field}={value}"),
            ]);
        }
    }
    args
}

fn provider_endpoint(provider: &str) -> Option<&'static str> {
    match crate::credentials::canonical_provider(provider) {
        "openai" => Some("https://api.openai.com/v1"),
        "anthropic" => Some("https://api.anthropic.com/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "nous" => Some("https://inference-api.nousresearch.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "kimi" => Some("https://api.moonshot.ai/v1"),
        "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai/"),
        _ => None,
    }
}

#[cfg(test)]
mod claude_subscription_tests {
    use super::*;

    #[test]
    fn selected_subscription_survives_while_paid_and_ambient_auth_are_removed() {
        let selected = vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), "private-token".into())];
        let mut env = vec![
            ("ANTHROPIC_API_KEY".into(), "paid-key".into()),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), "private-token".into()),
            ("CLAUDE_CODE_OAUTH_REFRESH_TOKEN".into(), "ambient".into()),
            ("CLAUDE_CODE_USE_BEDROCK".into(), "1".into()),
        ];
        filter_claude_selected_env(&mut env, &selected);
        assert_eq!(env, selected);
    }
}
