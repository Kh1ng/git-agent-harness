use super::ExecutionIdentity;
use anyhow::{bail, Result};
use std::path::Path;

impl ExecutionIdentity {
    /// File settings are checked before a child starts. The Python guard also
    /// checks actual provider resolution after in-memory admin configuration.
    pub(super) fn validate_vibe_config(&self, cwd: &Path) -> Result<()> {
        let root = self
            .state_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("named Vibe credentials require isolated state"))?;
        let mut paths = vec![root.join(".vibe/config.toml")];
        paths.extend(
            cwd.ancestors()
                .take_while(|directory| *directory != root)
                .map(|directory| directory.join(".vibe/config.toml")),
        );
        for path in paths {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => bail!("cannot validate Vibe credential configuration"),
            };
            let config: toml::Value = toml::from_str(&text)
                .map_err(|_| anyhow::anyhow!("cannot validate Vibe credential configuration"))?;
            if config.get("vibe_base_url").is_some_and(|value| {
                value.as_str().map(|url| url.trim_end_matches('/'))
                    != Some("https://chat.mistral.ai")
            }) {
                bail!("Vibe configuration overrides the selected credential provider");
            }
            if let Some(providers) = config.get("providers") {
                let providers = providers.as_array().ok_or_else(|| {
                    anyhow::anyhow!("cannot validate Vibe provider configuration")
                })?;
                for provider in providers {
                    if provider.get("name").and_then(toml::Value::as_str) != Some("mistral")
                        || provider.get("api_base").is_some_and(|value| {
                            value.as_str().map(|url| url.trim_end_matches('/'))
                                != Some("https://api.mistral.ai/v1")
                        })
                        || provider
                            .get("api_key_env_var")
                            .is_some_and(|value| value.as_str() != Some("MISTRAL_API_KEY"))
                        || provider.get("extra_headers").is_some_and(|value| {
                            value.as_table().is_none_or(|headers| !headers.is_empty())
                        })
                    {
                        bail!("Vibe configuration overrides the selected credential provider");
                    }
                }
            }
            for field in [
                "models",
                "compaction_model",
                "vision_model",
                "routed_model_config",
                "routed_extra_models",
            ] {
                if let Some(value) = config.get(field) {
                    validate_models(value)?;
                }
            }
        }
        Ok(())
    }
}

fn validate_models(value: &toml::Value) -> Result<()> {
    if value
        .get("provider")
        .is_some_and(|provider| provider.as_str() != Some("mistral"))
    {
        bail!("Vibe configuration overrides the selected credential provider");
    }
    match value {
        toml::Value::Array(models) => {
            for model in models {
                validate_models(model)?;
            }
        }
        toml::Value::Table(models) => {
            for model in models.values() {
                if model.is_table() {
                    validate_models(model)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
