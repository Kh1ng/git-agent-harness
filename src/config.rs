use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct GahConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Defaults {
    #[serde(default)]
    pub artifact_root: String,
    #[serde(default)]
    pub worktree_base: String,
    #[serde(default)]
    pub llm_base_url: String,
    #[serde(default)]
    pub llm_model_local: String,
    #[serde(default)]
    pub llm_model_cloud: String,
}

impl Defaults {
    pub fn llm_base_url(&self) -> String {
        std::env::var("LLM_BASE_URL").unwrap_or_else(|_| self.llm_base_url.clone())
    }
    pub fn llm_api_key(&self) -> String {
        std::env::var("LLM_API_KEY").unwrap_or_default()
    }
    pub fn llm_model(&self, cloud: bool) -> String {
        if let Ok(m) = std::env::var("LLM_MODEL") {
            return m;
        }
        if cloud {
            self.llm_model_cloud.clone()
        } else {
            self.llm_model_local.clone()
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Profile {
    pub display_name: String,
    pub repo_id: String,
    pub provider: String,
    pub repo: String,
    pub local_path: String,
    pub artifact_root: String,
    pub default_target_branch: String,
    #[serde(default)]
    pub provider_api_base: Option<String>,
    #[serde(default)]
    pub provider_project_id: Option<String>,
}

impl Profile {
    pub fn pat(&self) -> String {
        match self.provider.as_str() {
            "gitlab" => std::env::var("GITLAB_PAT2")
                .or_else(|_| std::env::var("GITLAB_PAT"))
                .unwrap_or_default(),
            "github" => std::env::var("GITHUB_TOKEN")
                .or_else(|_| std::env::var("GH_TOKEN"))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub fn push_url(&self) -> String {
        match self.provider.as_str() {
            "gitlab" => {
                let base = self.provider_api_base.as_deref().unwrap_or("");
                let host = base.trim_end_matches("/api/v4").trim_end_matches('/');
                let pat = self.pat();
                if let Some(rest) = host.strip_prefix("https://") {
                    format!("https://oauth2:{}@{}/{}.git", pat, rest, self.repo)
                } else if let Some(rest) = host.strip_prefix("http://") {
                    format!("http://oauth2:{}@{}/{}.git", pat, rest, self.repo)
                } else {
                    format!("{}/{}.git", host, self.repo)
                }
            }
            "github" => format!("https://github.com/{}.git", self.repo),
            _ => self.repo.clone(),
        }
    }
}

pub fn load(config_path: Option<&str>) -> Result<GahConfig> {
    let path = config_path
        .map(PathBuf::from)
        .or_else(|| std::env::var("GAH_CONFIG").ok().map(PathBuf::from))
        .or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            [
                PathBuf::from("/root/agent-lab/config/gah-config.toml"),
                PathBuf::from(format!("{}/.config/gah/config.toml", home)),
            ]
            .into_iter()
            .find(|p| p.exists())
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no config found; set GAH_CONFIG or create ~/.config/gah/config.toml")
        })?;

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn get_profile<'a>(config: &'a GahConfig, name: &str) -> Result<&'a Profile> {
    config.profiles.get(name).ok_or_else(|| {
        let mut names: Vec<&str> = config.profiles.keys().map(String::as_str).collect();
        names.sort_unstable();
        anyhow::anyhow!(
            "profile '{}' not found; available: {}",
            name,
            names.join(", ")
        )
    })
}
