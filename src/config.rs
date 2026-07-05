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
    #[serde(default)]
    pub routing: RoutingPolicy,
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

    pub fn ledger_path(&self) -> PathBuf {
        if let Ok(path) = std::env::var("GAH_LEDGER_PATH") {
            return PathBuf::from(path);
        }
        if !self.artifact_root.trim().is_empty() {
            return PathBuf::from(self.artifact_root.trim()).join("ledger.jsonl");
        }
        default_config_dir().join("ledger.jsonl")
    }

    /// TICKET-072: separate append-only log from `ledger.jsonl` (never
    /// rewrites dispatch history), same directory/override convention as
    /// `GAH_LEDGER_PATH`.
    pub fn reconciliation_path(&self) -> PathBuf {
        if let Ok(path) = std::env::var("GAH_RECONCILIATION_PATH") {
            return PathBuf::from(path);
        }
        if !self.artifact_root.trim().is_empty() {
            return PathBuf::from(self.artifact_root.trim()).join("reconciliation.jsonl");
        }
        default_config_dir().join("reconciliation.jsonl")
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
    /// Extra CLI args appended to `codex exec` for invariant non-model flags.
    #[serde(default)]
    pub codex_args: Vec<String>,
    /// Optional absolute/relative path to the Codex CLI executable.
    #[serde(default)]
    pub codex_path: Option<String>,
    /// Extra CLI args appended to `claude -p` (e.g. `--allowedTools Edit,Write,Bash`)
    #[serde(default)]
    pub claude_args: Vec<String>,
    /// Optional absolute/relative path to the Claude CLI executable.
    #[serde(default)]
    pub claude_path: Option<String>,
    /// Optional absolute/relative path to the Antigravity CLI executable.
    #[serde(default)]
    pub agy_path: Option<String>,
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
    /// TICKET-110/111: substrings that explicitly mark a baseline validation
    /// failure as known/expected (case-insensitive). Never inferred by the
    /// classifier itself -- only reachable via this explicit configuration.
    #[serde(default)]
    pub known_baseline_failure_markers: Vec<String>,
    /// Model override for `improve`/`fix` mode (heavy lifting)
    #[serde(default)]
    pub model_improve: Option<String>,
    /// Model override for `pm` mode (ticket decomposition, cheap/fast)
    #[serde(default)]
    pub model_pm: Option<String>,
    /// Model override for `review` mode
    #[serde(default)]
    pub model_review: Option<String>,
    /// Review subprocess timeout. Defaults to 300 seconds when unset.
    #[serde(default)]
    pub review_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub routing: RoutingPolicy,
    #[serde(default)]
    #[allow(dead_code)]
    pub pacing: crate::quota::PacingConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CandidateConfig {
    pub backend: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub included_in_quota: bool,
    #[serde(default)]
    pub marginal_cost_usd: Option<f64>,
    #[serde(default)]
    pub quota_usage_percent: Option<f64>,
    #[serde(default)]
    pub quota_days_remaining: Option<f64>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct RoutingPolicy {
    #[serde(default)]
    pub default_backend: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub pm_backend: Option<String>,
    #[serde(default)]
    pub pm_model: Option<String>,
    #[serde(default)]
    pub improve_backend: Option<String>,
    #[serde(default)]
    pub improve_model: Option<String>,
    #[serde(default)]
    pub review_backend: Option<String>,
    #[serde(default)]
    pub review_model: Option<String>,
    #[serde(default)]
    pub strong_review_backend: Option<String>,
    #[serde(default)]
    pub strong_review_model: Option<String>,
    #[serde(default)]
    pub weak_review_backend: Option<String>,
    #[serde(default)]
    pub weak_review_model: Option<String>,
    /// TICKET-109: capabilities required for review, keyed by backend name
    /// (e.g. `{"claude": ["ponytail"]}`). Checked at preflight (TICKET-105)
    /// and activated in the review prompt -- missing a required capability
    /// is a hard stop, never a silent downgrade to an ordinary review.
    #[serde(default)]
    pub review_required_capabilities: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub pm_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub improve_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub review_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub allow_review_fallback: bool,
    #[serde(default)]
    pub allow_implementation_fallback: bool,
    #[serde(default)]
    pub max_runs_per_backend_per_week: Option<u64>,
    #[serde(default)]
    pub max_runs_per_backend_per_session: Option<u64>,
    #[serde(default)]
    pub max_total_strong_model_runs_per_week: Option<u64>,
    #[serde(default)]
    pub max_total_strong_model_runs_per_session: Option<u64>,
    #[serde(default)]
    pub max_known_estimated_cost_per_week: Option<f64>,
    #[serde(default)]
    pub max_known_actual_cost_per_week: Option<f64>,
}

impl RoutingPolicy {
    pub fn find_quota_pool(
        &self,
        mode: &str,
        backend: &str,
        model: Option<&str>,
    ) -> Option<String> {
        let candidates = match mode {
            "pm" => self.pm_candidates.as_ref(),
            "review" => self.review_candidates.as_ref(),
            "improve" | "fix" | "experiment" => self.improve_candidates.as_ref(),
            _ => None,
        };
        if let Some(list) = candidates {
            for c in list {
                if c.backend == backend && c.model.as_deref() == model {
                    return c.quota_pool.clone();
                }
            }
        }
        None
    }
}

impl Profile {
    pub fn configured_backend_path(&self, backend: &str) -> Option<&str> {
        match backend {
            "codex" => self.codex_path.as_deref(),
            "claude" => self.claude_path.as_deref(),
            "agy" | "agy-main" | "agy-second" => self.agy_path.as_deref(),
            _ => None,
        }
    }

    pub fn review_timeout_seconds(&self) -> u64 {
        self.review_timeout_seconds.unwrap_or(300).max(1)
    }

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

    pub fn pat_env_names(&self) -> &'static [&'static str] {
        match self.provider.as_str() {
            "gitlab" => &["GITLAB_PAT2", "GITLAB_PAT"],
            "github" => &["GITHUB_TOKEN", "GH_TOKEN"],
            _ => &[],
        }
    }

    pub fn provider_cli(&self) -> Option<&'static str> {
        match self.provider.as_str() {
            "gitlab" => Some("glab"),
            "github" => Some("gh"),
            _ => None,
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

pub fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".config/gah")
}

pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

pub fn resolve_config_path(config_path: Option<&str>) -> PathBuf {
    config_path
        .map(PathBuf::from)
        .or_else(|| std::env::var("GAH_CONFIG").ok().map(PathBuf::from))
        .unwrap_or_else(default_config_path)
}

pub fn load(config_path: Option<&str>) -> Result<GahConfig> {
    let path = resolve_config_path(config_path);
    if !path.exists() {
        anyhow::bail!(
            "no config found; set GAH_CONFIG or create {}",
            default_config_path().display()
        );
    }

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
    use super::{Profile, RoutingPolicy};

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
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
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
