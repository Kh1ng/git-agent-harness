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
    /// OpenHands profile name (~/.openhands/profiles/<name>.json). Overrides default LLM config.
    #[serde(default)]
    pub oh_profile: Option<String>,
    /// Extra CLI args appended to the openhands invocation (e.g. plugins, skill flags)
    #[serde(default)]
    pub openhands_args: Vec<String>,
    /// Extra CLI args appended to `codex exec` (e.g. `-c model=gpt-4o`)
    #[serde(default)]
    pub codex_args: Vec<String>,
    /// Extra CLI args appended to `claude -p` (e.g. `--allowedTools Edit,Write,Bash`)
    #[serde(default)]
    pub claude_args: Vec<String>,
    /// Path to a policy TOML file (see gah policy-check). When set, dispatch
    /// enforces permissions before provisioning any worktree.
    #[serde(default)]
    pub policy_path: Option<String>,
    /// Optional path to a KEY=VALUE env file sourced before running any backend
    /// in dev mode (default). Contains dev/api keys, never prod credentials.
    #[serde(default)]
    pub env_file: Option<String>,
    /// Path to a production KEY=VALUE env file. Only loaded when --prod is passed
    /// to dispatch. Keeps prod credentials isolated from dev runs.
    #[serde(default)]
    pub env_file_prod: Option<String>,
    /// Commands run in the worktree after each agent attempt; all must pass before commit/push.
    /// Example: ["cargo test --quiet", "cargo clippy -- -D warnings"]
    #[serde(default)]
    pub validation_commands: Vec<String>,
    #[serde(default)]
    pub test_file_patterns: Vec<String>,
    /// Model override for `improve`/`fix` mode (heavy lifting)
    #[serde(default)]
    pub model_improve: Option<String>,
    /// Model override for `pm` mode (ticket decomposition, cheap/fast)
    #[serde(default)]
    pub model_pm: Option<String>,
    /// Model override for `review` mode
    #[serde(default)]
    pub model_review: Option<String>,
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

    /// Build push URL without embedding PAT. Authentication is handled
    /// via GIT_ASKPASS by the caller, so the token never appears in process
    /// arguments, process lists, or shell history.
    pub fn push_url(&self) -> Result<String> {
        match self.provider.as_str() {
            "gitlab" => {
                let base = self.gitlab_push_base()?;
                Ok(format!("{}/{}", base, normalize_repo_path(&self.repo)))
            }
            "github" => Ok(format!(
                "https://github.com/{}",
                normalize_repo_path(&self.repo)
            )),
            _ => Ok(self.repo.clone()),
        }
    }

    fn gitlab_push_base(&self) -> Result<String> {
        let base = self
            .provider_api_base
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?
            .trim();
        if base.is_empty() {
            anyhow::bail!("profile missing provider_api_base for gitlab");
        }

        let trimmed = base.trim_end_matches('/');
        let without_api = trimmed.strip_suffix("/api/v4").unwrap_or(trimmed);
        let (scheme, rest) = without_api
            .split_once("://")
            .unwrap_or(("https", without_api));
        let host = rest.split('/').next().unwrap_or("").trim_matches('/');
        if host.is_empty() {
            anyhow::bail!("invalid provider_api_base for gitlab: {}", base);
        }
        Ok(format!("{}://oauth2@{}", scheme, host))
    }
}

fn normalize_repo_path(repo: &str) -> String {
    let repo = repo.trim_matches('/');
    if repo.ends_with(".git") {
        repo.to_string()
    } else {
        format!("{}.git", repo)
    }
}

pub fn load(config_path: Option<&str>) -> Result<GahConfig> {
    let path = config_path
        .map(PathBuf::from)
        .or_else(|| std::env::var("GAH_CONFIG").ok().map(PathBuf::from))
        .or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            let p = PathBuf::from(format!("{}/.config/gah/config.toml", home));
            p.exists().then_some(p)
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

#[cfg(test)]
mod tests {
    use super::Profile;

    fn gitlab_profile(api_base: Option<&str>) -> Profile {
        Profile {
            display_name: "Test".into(),
            repo_id: "test".into(),
            provider: "gitlab".into(),
            repo: "group/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: api_base.map(str::to_string),
            provider_project_id: Some("42".into()),
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            claude_args: vec![],
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
        }
    }

    #[test]
    fn gitlab_push_url_uses_self_hosted_domain() {
        let profile = gitlab_profile(Some("https://gitlab.coltonspurgin.tech/api/v4"));
        assert_eq!(
            profile.push_url().unwrap(),
            "https://oauth2@gitlab.coltonspurgin.tech/group/repo.git"
        );
    }

    #[test]
    fn gitlab_push_url_handles_trailing_slash_and_missing_api_suffix() {
        let profile = gitlab_profile(Some("https://gitlab.example.com/"));
        assert_eq!(
            profile.push_url().unwrap(),
            "https://oauth2@gitlab.example.com/group/repo.git"
        );
    }

    #[test]
    fn gitlab_push_url_rejects_missing_host() {
        let profile = gitlab_profile(Some("https:///api/v4"));
        assert!(profile.push_url().is_err());
    }
}
