use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// TICKET-127/Issue #124: per-repo merge policy controlling what the
/// controller does once an MR is `READY_FOR_HUMAN` (strong reviewer approved)
/// and CI has been evaluated.
///
/// * `Auto` (default): current behavior -- strong review + green CI triggers
///   `MergeMr` (GAH merges itself).
/// * `StopForHuman`: strong review done + CI evaluated -> `HumanRequired`; GAH
///   never auto-merges, an operator clicks merge manually.
/// * `GitlabMwps`: after strong approval GAH sets GitLab's "merge when pipeline
///   succeeds" flag and does NOT merge itself; GitLab enforces the CI gate
///   natively. Only meaningful for GitLab; other providers fall back to `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MergePolicy {
    #[default]
    Auto,
    StopForHuman,
    GitlabMwps,
}

impl MergePolicy {
    /// Canonical config string for a merge policy (Issue #124 / TICKET-127).
    pub fn as_str(&self) -> &'static str {
        match self {
            MergePolicy::Auto => "auto",
            MergePolicy::StopForHuman => "stop_for_human",
            MergePolicy::GitlabMwps => "gitlab_mwps",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GahConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
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

    /// TICKET-083: append-only controller event stream, same convention as
    /// `GAH_LEDGER_PATH`/`GAH_RECONCILIATION_PATH`.
    pub fn events_path(&self) -> PathBuf {
        if let Ok(path) = std::env::var("GAH_EVENTS_PATH") {
            return PathBuf::from(path);
        }
        if !self.artifact_root.trim().is_empty() {
            return PathBuf::from(self.artifact_root.trim()).join("events.jsonl");
        }
        default_config_dir().join("events.jsonl")
    }
}

/// TICKET-128: per-profile policy for human-facing repository messaging.
///
/// This is an independent policy axis from reviewer routing and merge
/// authorization. It lets a profile (e.g. a workplace repo) keep full
/// autonomous code-execution + code-review capability while forbidding the
/// agent from authoring or publishing coworker-facing prose: PR/MR text,
/// generated commit messages, and issue-tracker comments.
///
/// All flags default to `true` so existing profiles keep their current
/// behavior unless they opt into a restricted profile explicitly.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct PublishingPolicy {
    /// When false, GAH must not create a PR/MR, and must not generate a
    /// PR/MR title or body as a fallback side effect. The run stops at a
    /// deterministic human handoff after code generation + validation.
    #[serde(default = "default_true")]
    pub allow_pull_request_creation: bool,
    /// When false, GAH must not ask an LLM to generate commit text nor
    /// synthesize prose commit messages from task context. The worktree is
    /// left uncommitted for human completion.
    #[serde(default = "default_true")]
    pub allow_commit_message_generation: bool,
    /// When false, GAH must not post status summaries, review findings,
    /// completion messages, or other agent-generated prose to issue trackers.
    #[serde(default = "default_true")]
    pub allow_issue_comments: bool,
    /// When false, reconciliation must never close a source issue after a
    /// merged PR/MR, even when authoritative closure evidence exists.
    /// Defaults to false so this new remote-write path is opt-in.
    #[serde(default)]
    pub allow_source_issue_closure: bool,
}

impl Default for PublishingPolicy {
    fn default() -> Self {
        Self {
            allow_pull_request_creation: true,
            allow_commit_message_generation: true,
            allow_issue_comments: true,
            allow_source_issue_closure: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
    /// Extra CLI args appended to `vibe -p` (e.g. `--max-turns 40 --max-price 2`).
    /// Worker/fix backend only -- not wired into review.
    #[serde(default)]
    pub vibe_args: Vec<String>,
    /// Optional absolute/relative path to the Mistral Vibe CLI executable.
    #[serde(default)]
    pub vibe_path: Option<String>,
    /// Extra CLI args appended to `opencode run` (e.g. `--format json`).
    /// Worker/fix backend only -- not wired into review.
    #[serde(default)]
    pub opencode_args: Vec<String>,
    /// Optional absolute/relative path to the OpenCode CLI executable.
    #[serde(default)]
    pub opencode_path: Option<String>,
    /// How long OpenCode's own log output can go quiet before GAH considers
    /// it stalled and kills it, in seconds. Same rationale as
    /// `agy_idle_timeout_seconds` (opencode's own multi-step sub-agent
    /// orchestration can legitimately pause longer between visible output
    /// than a single-shot backend, hence the more generous default) --
    /// added after a live dispatch hung for 3+ hours with zero output and
    /// no supervision at all (opencode had no timeout of any kind before
    /// this). Defaults to 300s when unset.
    #[serde(default)]
    pub opencode_idle_timeout_seconds: Option<u64>,
    /// How long OpenHands' own log output can go quiet before GAH considers
    /// it stalled and kills it, in seconds. Same rationale and mechanism as
    /// `opencode_idle_timeout_seconds` -- added after a live dispatch (issue
    /// #87) ran for 2h20m+ with openhands as the backend and `run_openhands`
    /// had zero supervision of any kind (a plain blocking `cmd.status()`).
    /// Defaults to 300s when unset.
    #[serde(default)]
    pub openhands_idle_timeout_seconds: Option<u64>,
    /// HOME override for the `agy-second` backend name only -- a distinct
    /// authenticated Antigravity account/quota pool from the default `agy`
    /// backend, which otherwise runs under the process's real $HOME. Same
    /// executable (`agy_path`), different account state directory.
    #[serde(default)]
    pub agy_second_home: Option<String>,
    /// Per-model override for AGY's own `--print-timeout` (default 5m0s in
    /// the `agy` CLI itself). Keyed by the exact AGY model name (e.g.
    /// "Gemini 3.5 Flash (Medium)"). This is now an outer safety backstop
    /// only -- the primary enforcement is `agy_idle_timeout_seconds` below,
    /// which kills based on whether AGY is still producing output, not a
    /// flat wall-clock budget. AGY is the only backend GAH invokes that
    /// exposes a print-timeout flag today, so this stays scoped to AGY
    /// rather than a generic cross-backend timeout abstraction.
    #[serde(default)]
    pub agy_print_timeout_seconds: HashMap<String, u64>,
    /// How long AGY's own log output can go quiet before GAH considers it
    /// stalled and kills it, in seconds. Deliberately not per-model: a
    /// working model of any speed should produce *some* log output
    /// periodically as it takes actions, so a flat idle threshold is the
    /// right granularity here (unlike agy_print_timeout_seconds, which is
    /// about total budget and genuinely varies by model). Defaults to 120s
    /// when unset (see `Profile::agy_idle_timeout_seconds`).
    #[serde(default)]
    pub agy_idle_timeout_seconds: Option<u64>,
    /// Optional shell command that GAH pipes a one-line notification message to
    /// (via stdin) on key controller/dispatch events (MR created, human required,
    /// review verdict, terminal dispatch failure). When unset GAH produces no
    /// notification output at all. Notification failures are always swallowed and
    /// logged to stderr -- they never fail the operation.
    #[serde(default)]
    pub notify_command: Option<String>,
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
    /// Mechanical formatting fixups run (best-effort, failures ignored) in
    /// the worktree immediately before `validation_commands`, on every
    /// attempt. Example: ["cargo fmt"]. Backends routinely write correct
    /// but unformatted code and burn a full retry (LLM call + review) on
    /// a `cargo fmt --check`/`black --check`-style failure that a
    /// deterministic formatter would have fixed in milliseconds -- this
    /// runs the formatter instead of retrying for it.
    #[serde(default)]
    pub auto_fix_commands: Vec<String>,
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
    /// TICKET-128: per-profile policy for human-facing repository messaging.
    /// Independence axis from reviewer routing and merge authorization: a
    /// restricted profile can keep full code-execution + review capability
    /// while forbidding agent-authored PR/MR text, commit messages, and
    /// issue-tracker comments. Defaults to everything allowed.
    #[serde(default)]
    pub publishing: PublishingPolicy,
    #[serde(default)]
    #[allow(dead_code)]
    pub pacing: crate::quota::PacingConfig,
    /// TICKET-158: per-profile retention window (days) for `gah prune`.
    /// High-churn self-hosting profiles can prune aggressively (e.g. 3-7)
    /// while low-churn profiles keep the 30-day default. The CLI
    /// `--older-than` flag overrides this per invocation.
    #[serde(default)]
    pub prune_older_than_days: Option<u64>,
}

impl Profile {
    /// Effective worktree/session retention window in days for `gah prune`.
    /// Falls back to 30 when the profile does not set `prune_older_than_days`.
    pub fn effective_prune_older_than_days(&self) -> u64 {
        self.prune_older_than_days.unwrap_or(30)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
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

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
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
    /// TICKET-127/Issue #124: per-repo merge policy gating what the
    /// controller does for a `READY_FOR_HUMAN` MR whose CI has been evaluated.
    /// `None` inherits the canonical/defaults policy (resolved to `Auto`).
    #[serde(default)]
    pub merge_policy: Option<MergePolicy>,
}

impl RoutingPolicy {
    pub fn merged_with_defaults(&self, defaults: &RoutingPolicy) -> RoutingPolicy {
        RoutingPolicy {
            default_backend: self
                .default_backend
                .clone()
                .or_else(|| defaults.default_backend.clone()),
            default_model: self
                .default_model
                .clone()
                .or_else(|| defaults.default_model.clone()),
            pm_backend: self
                .pm_backend
                .clone()
                .or_else(|| defaults.pm_backend.clone()),
            pm_model: self.pm_model.clone().or_else(|| defaults.pm_model.clone()),
            improve_backend: self
                .improve_backend
                .clone()
                .or_else(|| defaults.improve_backend.clone()),
            improve_model: self
                .improve_model
                .clone()
                .or_else(|| defaults.improve_model.clone()),
            review_backend: self
                .review_backend
                .clone()
                .or_else(|| defaults.review_backend.clone()),
            review_model: self
                .review_model
                .clone()
                .or_else(|| defaults.review_model.clone()),
            strong_review_backend: self
                .strong_review_backend
                .clone()
                .or_else(|| defaults.strong_review_backend.clone()),
            strong_review_model: self
                .strong_review_model
                .clone()
                .or_else(|| defaults.strong_review_model.clone()),
            weak_review_backend: self
                .weak_review_backend
                .clone()
                .or_else(|| defaults.weak_review_backend.clone()),
            weak_review_model: self
                .weak_review_model
                .clone()
                .or_else(|| defaults.weak_review_model.clone()),
            pm_candidates: self
                .pm_candidates
                .clone()
                .or_else(|| defaults.pm_candidates.clone()),
            improve_candidates: self
                .improve_candidates
                .clone()
                .or_else(|| defaults.improve_candidates.clone()),
            review_candidates: self
                .review_candidates
                .clone()
                .or_else(|| defaults.review_candidates.clone()),
            review_required_capabilities: {
                // Same merge-by-key semantics as merge_routing_policy (TICKET-106):
                // repo's own entries win for a given key, defaults fill the rest.
                let mut merged = defaults.review_required_capabilities.clone();
                merged.extend(self.review_required_capabilities.clone());
                merged
            },
            allow_review_fallback: self.allow_review_fallback || defaults.allow_review_fallback,
            allow_implementation_fallback: self.allow_implementation_fallback
                || defaults.allow_implementation_fallback,
            max_runs_per_backend_per_week: self
                .max_runs_per_backend_per_week
                .or(defaults.max_runs_per_backend_per_week),
            max_runs_per_backend_per_session: self
                .max_runs_per_backend_per_session
                .or(defaults.max_runs_per_backend_per_session),
            max_total_strong_model_runs_per_week: self
                .max_total_strong_model_runs_per_week
                .or(defaults.max_total_strong_model_runs_per_week),
            max_total_strong_model_runs_per_session: self
                .max_total_strong_model_runs_per_session
                .or(defaults.max_total_strong_model_runs_per_session),
            max_known_estimated_cost_per_week: self
                .max_known_estimated_cost_per_week
                .or(defaults.max_known_estimated_cost_per_week),
            max_known_actual_cost_per_week: self
                .max_known_actual_cost_per_week
                .or(defaults.max_known_actual_cost_per_week),
            merge_policy: self.merge_policy.or(defaults.merge_policy),
        }
    }

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
    pub fn effective_routing(&self, defaults: &Defaults) -> RoutingPolicy {
        self.routing.merged_with_defaults(&defaults.routing)
    }

    /// An explicit executable path override for `backend`, if this profile
    /// sets one. `resolve_backend_executable` (runner.rs) treats a `Some`
    /// return as a literal file path to check with `is_executable_path` --
    /// this must ONLY ever return a real path override, never a marker
    /// string, or backend launch silently breaks (see `is_backend_configured`
    /// below for the "is this set up at all" signal, which is a different
    /// question with a different answer for openhands).
    pub fn configured_backend_path(&self, backend: &str) -> Option<&str> {
        match backend {
            "codex" => self.codex_path.as_deref(),
            "claude" => self.claude_path.as_deref(),
            "agy" | "agy-main" | "agy-second" => self.agy_path.as_deref(),
            "vibe" => self.vibe_path.as_deref(),
            "opencode" => self.opencode_path.as_deref(),
            _ => None,
        }
    }

    /// Whether `backend` is set up for this profile at all -- distinct from
    /// `configured_backend_path`, which only reports an *explicit path
    /// override* and is consumed by `resolve_backend_executable` to find the
    /// literal binary to run. OpenHands has no such override (its CLI is
    /// always resolved on PATH); its "configured" signal is instead whether
    /// this profile sets an `oh_profile`. Settings' "configured for this
    /// profile" display (issue #157) should call this, not
    /// `configured_backend_path`, for exactly this reason -- an earlier
    /// version conflated the two and made openhands's `oh_profile` name
    /// (e.g. "nous-hy3") look like an executable path to
    /// `resolve_backend_executable`, which then failed `is_executable_path`
    /// and made routing treat openhands as unavailable for every explicit
    /// `--backend openhands` dispatch.
    pub fn is_backend_configured(&self, backend: &str) -> bool {
        if backend == "openhands" {
            return self.oh_profile.is_some();
        }
        self.configured_backend_path(backend).is_some()
    }

    pub fn review_timeout_seconds(&self) -> u64 {
        self.review_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn agy_idle_timeout_seconds(&self) -> u64 {
        self.agy_idle_timeout_seconds.unwrap_or(120).max(1)
    }

    pub fn opencode_idle_timeout_seconds(&self) -> u64 {
        self.opencode_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn openhands_idle_timeout_seconds(&self) -> u64 {
        self.openhands_idle_timeout_seconds.unwrap_or(300).max(1)
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

    /// Human-facing repo URL (for linking out from the frontend/CLI), as
    /// opposed to `push_url()` which embeds an oauth2@ placeholder and a
    /// `.git` suffix meant for git itself, not a browser.
    pub fn web_url(&self) -> Option<String> {
        match self.provider.as_str() {
            "github" => Some(format!(
                "https://github.com/{}",
                self.repo.trim_matches('/')
            )),
            "gitlab" => {
                let base = self.gitlab_push_base().ok()?;
                let host = base.split_once('@').map(|(_, host)| host).unwrap_or(&base);
                Some(format!("https://{}/{}", host, self.repo.trim_matches('/')))
            }
            _ => None,
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

/// TICKET-106: shared canonical routing policy, inherited by every repo's
/// config unless explicitly overridden. Separate file from the per-repo
/// config (`GAH_CANONICAL_CONFIG` override, else `~/.config/gah/canonical.toml`
/// -- same `~/.config/gah/` convention as the default repo config path).
pub fn canonical_config_path() -> PathBuf {
    std::env::var("GAH_CANONICAL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_config_dir().join("canonical.toml"))
}

#[derive(Debug, Deserialize, Default)]
struct CanonicalConfig {
    #[serde(default)]
    routing: RoutingPolicy,
}

/// `Ok(None)` when no canonical file exists (legacy standalone behavior).
/// Malformed canonical config is a hard error, never silently ignored --
/// an operator's shared policy must not silently fail to apply.
fn load_canonical_routing() -> Result<Option<RoutingPolicy>> {
    let path = canonical_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let canonical: CanonicalConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(canonical.routing))
}

/// Field-level merge: `repo`'s own explicit values win; unset fields
/// inherit from `canonical`. Candidate lists (Vec) replace wholesale when
/// the repo sets them (not concatenate); the capability map merges by key
/// so a repo declaring one backend's capabilities doesn't erase another
/// backend's canonical-declared ones.
fn merge_routing_policy(canonical: RoutingPolicy, mut repo: RoutingPolicy) -> RoutingPolicy {
    repo.default_backend = repo.default_backend.or(canonical.default_backend);
    repo.default_model = repo.default_model.or(canonical.default_model);
    repo.pm_backend = repo.pm_backend.or(canonical.pm_backend);
    repo.pm_model = repo.pm_model.or(canonical.pm_model);
    repo.improve_backend = repo.improve_backend.or(canonical.improve_backend);
    repo.improve_model = repo.improve_model.or(canonical.improve_model);
    repo.review_backend = repo.review_backend.or(canonical.review_backend);
    repo.review_model = repo.review_model.or(canonical.review_model);
    repo.strong_review_backend = repo
        .strong_review_backend
        .or(canonical.strong_review_backend);
    repo.strong_review_model = repo.strong_review_model.or(canonical.strong_review_model);
    repo.weak_review_backend = repo.weak_review_backend.or(canonical.weak_review_backend);
    repo.weak_review_model = repo.weak_review_model.or(canonical.weak_review_model);
    repo.pm_candidates = repo.pm_candidates.or(canonical.pm_candidates);
    repo.improve_candidates = repo.improve_candidates.or(canonical.improve_candidates);
    repo.review_candidates = repo.review_candidates.or(canonical.review_candidates);
    repo.allow_review_fallback = repo.allow_review_fallback || canonical.allow_review_fallback;
    repo.allow_implementation_fallback =
        repo.allow_implementation_fallback || canonical.allow_implementation_fallback;
    repo.max_runs_per_backend_per_week = repo
        .max_runs_per_backend_per_week
        .or(canonical.max_runs_per_backend_per_week);
    repo.max_runs_per_backend_per_session = repo
        .max_runs_per_backend_per_session
        .or(canonical.max_runs_per_backend_per_session);
    repo.max_total_strong_model_runs_per_week = repo
        .max_total_strong_model_runs_per_week
        .or(canonical.max_total_strong_model_runs_per_week);
    repo.max_total_strong_model_runs_per_session = repo
        .max_total_strong_model_runs_per_session
        .or(canonical.max_total_strong_model_runs_per_session);
    repo.max_known_estimated_cost_per_week = repo
        .max_known_estimated_cost_per_week
        .or(canonical.max_known_estimated_cost_per_week);
    repo.max_known_actual_cost_per_week = repo
        .max_known_actual_cost_per_week
        .or(canonical.max_known_actual_cost_per_week);
    let mut capabilities = canonical.review_required_capabilities;
    capabilities.extend(repo.review_required_capabilities);
    repo.review_required_capabilities = capabilities;
    repo.merge_policy = repo.merge_policy.or(canonical.merge_policy);
    repo
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
    let mut cfg: GahConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(canonical_routing) = load_canonical_routing()? {
        cfg.defaults.routing = merge_routing_policy(canonical_routing, cfg.defaults.routing);
    }
    Ok(cfg)
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

/// Save the config back to the TOML file
pub fn save(config: &GahConfig, path: Option<&str>) -> Result<()> {
    let target_path = resolve_config_path(path);

    // Ensure parent directory exists
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent).context("creating config directory")?;
    }

    let toml_string = toml::to_string(config).context("serializing config to TOML")?;
    std::fs::write(&target_path, toml_string).context("writing config file")?;
    Ok(())
}

/// Add a new profile to the config
pub fn add_profile(config: &mut GahConfig, name: &str, profile: Profile) -> Result<()> {
    if config.profiles.contains_key(name) {
        anyhow::bail!("profile '{}' already exists", name);
    }
    config.profiles.insert(name.to_string(), profile);
    Ok(())
}

/// Remove a profile from the config
pub fn remove_profile(config: &mut GahConfig, name: &str) -> Result<()> {
    if !config.profiles.contains_key(name) {
        anyhow::bail!("profile '{}' not found", name);
    }
    config.profiles.remove(name);
    Ok(())
}

/// Get a mutable reference to a profile for in-place modification
pub fn get_profile_mut<'a>(config: &'a mut GahConfig, name: &str) -> Result<&'a mut Profile> {
    if !config.profiles.contains_key(name) {
        let names: Vec<String> = config.profiles.keys().cloned().collect();
        anyhow::bail!(
            "profile '{}' not found; available: {}",
            name,
            names.join(", ")
        );
    }
    Ok(config.profiles.get_mut(name).unwrap())
}

#[cfg(test)]
pub mod tests {
    use super::{
        add_profile, get_profile_mut, load, load_canonical_routing, merge_routing_policy,
        remove_profile, save, CandidateConfig, GahConfig, Profile, RoutingPolicy,
    };
    use std::sync::Mutex;

    /// Build a structurally complete `Profile` for unit tests in other modules
    /// (e.g. `notifications`). Mirrors the shape of `dispatch::tests::profile`
    /// so notification formatting tests can construct a real `Profile` without
    /// duplicating every field.
    #[cfg(test)]
    pub fn test_profile_for_notifications() -> Profile {
        Profile {
            prune_older_than_days: None,
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            openhands_idle_timeout_seconds: None,
            notify_command: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            publishing: Default::default(),
            pacing: Default::default(),
        }
    }

    // TICKET-106: GAH_CANONICAL_CONFIG is a process-global env var; every
    // test that touches it must go through this lock (same reasoning as
    // test_support::PathGuard for PATH) or parallel test threads corrupt
    // each other's view of it.
    static CANONICAL_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn gitlab_profile(api_base: Option<&str>) -> Profile {
        Profile {
            prune_older_than_days: None,
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
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            openhands_idle_timeout_seconds: None,
            notify_command: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            publishing: Default::default(),
            pacing: Default::default(),
        }
    }

    fn config_with_one_profile() -> GahConfig {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert("test".to_string(), test_profile_for_notifications());
        GahConfig {
            defaults: Default::default(),
            profiles,
        }
    }

    #[test]
    fn add_profile_inserts_new_entry() {
        let mut cfg = config_with_one_profile();
        add_profile(&mut cfg, "second", gitlab_profile(None)).unwrap();
        assert!(cfg.profiles.contains_key("second"));
        assert_eq!(cfg.profiles.len(), 2);
    }

    #[test]
    fn add_profile_rejects_duplicate_name() {
        let mut cfg = config_with_one_profile();
        let err = add_profile(&mut cfg, "test", gitlab_profile(None)).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // Original profile must be untouched by the rejected add.
        assert_eq!(cfg.profiles.get("test").unwrap().provider, "github");
    }

    #[test]
    fn remove_profile_deletes_existing_entry() {
        let mut cfg = config_with_one_profile();
        remove_profile(&mut cfg, "test").unwrap();
        assert!(!cfg.profiles.contains_key("test"));
    }

    #[test]
    fn remove_profile_errors_on_missing_name() {
        let mut cfg = config_with_one_profile();
        let err = remove_profile(&mut cfg, "nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
        // Nothing should have been removed.
        assert_eq!(cfg.profiles.len(), 1);
    }

    #[test]
    fn get_profile_mut_allows_in_place_field_update() {
        let mut cfg = config_with_one_profile();
        {
            let profile = get_profile_mut(&mut cfg, "test").unwrap();
            profile.display_name = "Renamed".to_string();
        }
        assert_eq!(cfg.profiles.get("test").unwrap().display_name, "Renamed");
    }

    #[test]
    fn get_profile_mut_errors_on_missing_name_and_lists_available() {
        let mut cfg = config_with_one_profile();
        let err = get_profile_mut(&mut cfg, "nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn save_round_trips_a_profile_through_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = config_with_one_profile();
        save(&cfg, Some(path.to_str().unwrap())).unwrap();

        let reloaded = load(Some(path.to_str().unwrap())).unwrap();
        let profile = reloaded.profiles.get("test").unwrap();
        assert_eq!(profile.display_name, "Repo");
        assert_eq!(profile.repo, "owner/repo");
    }

    // Regression: configured_backend_path("openhands") used to return
    // oh_profile (e.g. "nous-hy3"), which resolve_backend_executable
    // (runner.rs) then treated as a literal executable file path -- that
    // string is never a real file, so every explicit `--backend openhands`
    // dispatch on a profile with oh_profile set silently routed away from
    // openhands as if it were unavailable. configured_backend_path must
    // never return anything for openhands; is_backend_configured is the
    // right function for "is this set up" instead.
    #[test]
    fn configured_backend_path_never_returns_a_value_for_openhands() {
        let mut profile = test_profile_for_notifications();
        profile.oh_profile = Some("nous-hy3".to_string());
        assert_eq!(profile.configured_backend_path("openhands"), None);
    }

    #[test]
    fn is_backend_configured_true_for_openhands_with_oh_profile_set() {
        let mut profile = test_profile_for_notifications();
        profile.oh_profile = Some("nous-hy3".to_string());
        assert!(profile.is_backend_configured("openhands"));
    }

    #[test]
    fn is_backend_configured_false_for_openhands_without_oh_profile() {
        let profile = test_profile_for_notifications();
        assert_eq!(profile.oh_profile, None);
        assert!(!profile.is_backend_configured("openhands"));
    }

    #[test]
    fn is_backend_configured_delegates_to_path_for_other_backends() {
        let mut profile = test_profile_for_notifications();
        assert!(!profile.is_backend_configured("codex"));
        profile.codex_path = Some("/usr/local/bin/codex".to_string());
        assert!(profile.is_backend_configured("codex"));
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

    #[test]
    fn web_url_github_is_a_clickable_repo_link() {
        let profile = test_profile_for_notifications();
        assert_eq!(profile.web_url().unwrap(), "https://github.com/owner/repo");
    }

    #[test]
    fn web_url_gitlab_uses_self_hosted_domain_without_credentials_or_git_suffix() {
        let profile = gitlab_profile(Some("https://gitlab.coltonspurgin.tech/api/v4"));
        assert_eq!(
            profile.web_url().unwrap(),
            "https://gitlab.coltonspurgin.tech/group/repo"
        );
    }

    #[test]
    fn web_url_none_when_provider_unrecognized_or_gitlab_misconfigured() {
        let mut profile = gitlab_profile(None);
        assert!(profile.web_url().is_none());
        profile.provider = "bitbucket".into();
        assert!(profile.web_url().is_none());
    }

    #[test]
    fn merge_keeps_repo_value_when_both_set() {
        let canonical = RoutingPolicy {
            default_backend: Some("codex".into()),
            ..Default::default()
        };
        let repo = RoutingPolicy {
            default_backend: Some("claude".into()),
            ..Default::default()
        };
        let merged = merge_routing_policy(canonical, repo);
        assert_eq!(merged.default_backend.as_deref(), Some("claude"));
    }

    #[test]
    fn merge_fills_gap_from_canonical_when_repo_unset() {
        let canonical = RoutingPolicy {
            default_backend: Some("codex".into()),
            review_backend: Some("claude".into()),
            ..Default::default()
        };
        let repo = RoutingPolicy::default();
        let merged = merge_routing_policy(canonical, repo);
        assert_eq!(merged.default_backend.as_deref(), Some("codex"));
        assert_eq!(merged.review_backend.as_deref(), Some("claude"));
    }

    #[test]
    fn merge_nested_override_does_not_erase_unrelated_inherited_fields() {
        // TICKET-106 AC: "one nested override does not erase unrelated
        // inherited values" -- repo overrides only default_backend, and
        // review_backend must still come from canonical.
        let canonical = RoutingPolicy {
            default_backend: Some("codex".into()),
            review_backend: Some("claude".into()),
            ..Default::default()
        };
        let repo = RoutingPolicy {
            default_backend: Some("agy".into()),
            ..Default::default()
        };
        let merged = merge_routing_policy(canonical, repo);
        assert_eq!(merged.default_backend.as_deref(), Some("agy"));
        assert_eq!(merged.review_backend.as_deref(), Some("claude"));
    }

    #[test]
    fn merge_candidate_list_replaces_wholesale_not_concatenates() {
        let canonical = RoutingPolicy {
            improve_candidates: Some(vec![CandidateConfig {
                backend: "codex".into(),
                model: None,
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            }]),
            ..Default::default()
        };
        let repo = RoutingPolicy {
            improve_candidates: Some(vec![CandidateConfig {
                backend: "claude".into(),
                model: None,
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            }]),
            ..Default::default()
        };
        let merged = merge_routing_policy(canonical, repo);
        let candidates = merged.improve_candidates.unwrap();
        assert_eq!(candidates.len(), 1, "replace, not concatenate");
        assert_eq!(candidates[0].backend, "claude");
    }

    #[test]
    fn merge_capability_map_merges_by_key() {
        use std::collections::HashMap;
        let mut canonical_caps = HashMap::new();
        canonical_caps.insert("claude".to_string(), vec!["ponytail".to_string()]);
        let canonical = RoutingPolicy {
            review_required_capabilities: canonical_caps,
            ..Default::default()
        };
        let mut repo_caps = HashMap::new();
        repo_caps.insert("codex".to_string(), vec!["something-else".to_string()]);
        let repo = RoutingPolicy {
            review_required_capabilities: repo_caps,
            ..Default::default()
        };
        let merged = merge_routing_policy(canonical, repo);
        assert_eq!(
            merged.review_required_capabilities.get("claude"),
            Some(&vec!["ponytail".to_string()])
        );
        assert_eq!(
            merged.review_required_capabilities.get("codex"),
            Some(&vec!["something-else".to_string()])
        );
    }

    #[test]
    fn merge_capability_map_repo_key_overrides_canonical_same_key() {
        use std::collections::HashMap;
        let mut canonical_caps = HashMap::new();
        canonical_caps.insert("claude".to_string(), vec!["ponytail".to_string()]);
        let canonical = RoutingPolicy {
            review_required_capabilities: canonical_caps,
            ..Default::default()
        };
        let mut repo_caps = HashMap::new();
        repo_caps.insert("claude".to_string(), vec![]);
        let repo = RoutingPolicy {
            review_required_capabilities: repo_caps,
            ..Default::default()
        };
        let merged = merge_routing_policy(canonical, repo);
        assert_eq!(
            merged.review_required_capabilities.get("claude"),
            Some(&vec![])
        );
    }

    #[test]
    fn merge_with_all_default_canonical_is_a_no_op() {
        // TICKET-106 AC: "missing shared defaults preserves backward-
        // compatible behavior" -- an all-default canonical (equivalent to
        // no canonical file existing) must not change repo's routing at all.
        let repo = RoutingPolicy {
            default_backend: Some("codex".into()),
            allow_review_fallback: true,
            ..Default::default()
        };
        let merged = merge_routing_policy(RoutingPolicy::default(), repo.clone());
        assert_eq!(merged.default_backend, repo.default_backend);
        assert_eq!(merged.allow_review_fallback, repo.allow_review_fallback);
    }

    #[test]
    fn load_canonical_routing_is_none_when_file_does_not_exist() {
        let _lock = CANONICAL_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(
            "GAH_CANONICAL_CONFIG",
            tmp.path().join("does-not-exist.toml"),
        );
        let result = load_canonical_routing().unwrap();
        std::env::remove_var("GAH_CANONICAL_CONFIG");
        assert!(result.is_none());
    }

    #[test]
    fn load_canonical_routing_fails_loudly_on_malformed_file() {
        let _lock = CANONICAL_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("canonical.toml");
        std::fs::write(&path, "not valid toml [[[").unwrap();
        std::env::set_var("GAH_CANONICAL_CONFIG", &path);
        let result = load_canonical_routing();
        std::env::remove_var("GAH_CANONICAL_CONFIG");
        assert!(result.is_err());
    }

    #[test]
    fn load_merges_canonical_into_repo_defaults_routing() {
        let _lock = CANONICAL_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join("canonical.toml");
        std::fs::write(
            &canonical_path,
            "[routing]\ndefault_backend = \"codex\"\nreview_backend = \"claude\"\n",
        )
        .unwrap();
        std::env::set_var("GAH_CANONICAL_CONFIG", &canonical_path);

        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[defaults.routing]\ndefault_backend = \"agy\"\n",
        )
        .unwrap();

        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        std::env::remove_var("GAH_CANONICAL_CONFIG");

        // repo's own default_backend wins...
        assert_eq!(cfg.defaults.routing.default_backend.as_deref(), Some("agy"));
        // ...but review_backend, which repo never set, inherits from canonical.
        assert_eq!(
            cfg.defaults.routing.review_backend.as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn two_different_repo_configs_both_inherit_the_same_canonical_routing() {
        // TICKET-106 AC: "World Cup can inherit canonical routing while
        // overriding repo-specific behavior" -- simulated with two throwaway
        // repo configs rather than touching a real second repo.
        let _lock = CANONICAL_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join("canonical.toml");
        std::fs::write(
            &canonical_path,
            "[routing]\nreview_backend = \"claude\"\nstrong_review_backend = \"claude\"\n",
        )
        .unwrap();
        std::env::set_var("GAH_CANONICAL_CONFIG", &canonical_path);

        let minimal_repo_path = tmp.path().join("minimal-repo.toml");
        std::fs::write(
            &minimal_repo_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n",
        )
        .unwrap();

        let overriding_repo_path = tmp.path().join("overriding-repo.toml");
        std::fs::write(
            &overriding_repo_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[defaults.routing]\nreview_backend = \"codex\"\n",
        )
        .unwrap();

        let minimal_cfg = load(Some(minimal_repo_path.to_str().unwrap())).unwrap();
        let overriding_cfg = load(Some(overriding_repo_path.to_str().unwrap())).unwrap();
        std::env::remove_var("GAH_CANONICAL_CONFIG");

        // A minimal repo config with no routing section at all still
        // receives canonical routing automatically.
        assert_eq!(
            minimal_cfg.defaults.routing.review_backend.as_deref(),
            Some("claude")
        );
        assert_eq!(
            minimal_cfg
                .defaults
                .routing
                .strong_review_backend
                .as_deref(),
            Some("claude")
        );
        // A repo that overrides one field keeps its own value for that
        // field, but still inherits the other canonical field unchanged.
        assert_eq!(
            overriding_cfg.defaults.routing.review_backend.as_deref(),
            Some("codex")
        );
        assert_eq!(
            overriding_cfg
                .defaults
                .routing
                .strong_review_backend
                .as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn prune_older_than_days_defaults_to_30() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[profiles.repo]\ndisplay_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\n",
        )
        .unwrap();
        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        let profile = cfg.profiles.get("repo").unwrap();
        assert_eq!(profile.prune_older_than_days, None);
        assert_eq!(profile.effective_prune_older_than_days(), 30);
    }

    #[test]
    fn prune_older_than_days_respects_profile_override() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[profiles.repo]\ndisplay_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\nprune_older_than_days = 7\n",
        )
        .unwrap();
        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        let profile = cfg.profiles.get("repo").unwrap();
        assert_eq!(profile.prune_older_than_days, Some(7));
        assert_eq!(profile.effective_prune_older_than_days(), 7);
    }

    #[test]
    fn shipped_canonical_example_parses_as_a_canonical_config() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/config/gah-canonical.example.toml"
        ))
        .unwrap();
        let canonical: super::CanonicalConfig = toml::from_str(&text).unwrap();
        assert_eq!(
            canonical.routing.strong_review_backend.as_deref(),
            Some("claude")
        );
        assert_eq!(
            canonical.routing.review_required_capabilities.get("claude"),
            Some(&vec!["ponytail".to_string()])
        );
    }
    #[test]
    fn routing_policy_inherits_missing_fields_from_defaults() {
        let defaults = RoutingPolicy {
            default_backend: Some("codex".into()),
            pm_backend: Some("claude".into()),
            pm_candidates: Some(vec![super::CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet".into()),
                quota_pool: Some("claude-main".into()),
                priority: 2,
                included_in_quota: true,
                marginal_cost_usd: Some(0.0),
                quota_usage_percent: Some(25.0),
                quota_days_remaining: Some(5.0),
            }]),
            allow_review_fallback: true,
            max_runs_per_backend_per_week: Some(3),
            ..RoutingPolicy::default()
        };
        let profile = RoutingPolicy {
            improve_backend: Some("agy".into()),
            ..RoutingPolicy::default()
        };

        let merged = profile.merged_with_defaults(&defaults);

        assert_eq!(merged.default_backend.as_deref(), Some("codex"));
        assert_eq!(merged.pm_backend.as_deref(), Some("claude"));
        assert_eq!(merged.improve_backend.as_deref(), Some("agy"));
        assert_eq!(merged.pm_candidates.as_ref().map(Vec::len), Some(1));
        assert!(merged.allow_review_fallback);
        assert_eq!(merged.max_runs_per_backend_per_week, Some(3));
    }

    #[test]
    fn routing_policy_profile_candidate_list_replaces_default_list() {
        let defaults = RoutingPolicy {
            pm_candidates: Some(vec![super::CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            }]),
            ..RoutingPolicy::default()
        };
        let profile = RoutingPolicy {
            pm_candidates: Some(vec![super::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            }]),
            ..RoutingPolicy::default()
        };

        let merged = profile.merged_with_defaults(&defaults);

        let candidates = merged.pm_candidates.expect("merged candidate list");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].backend, "codex");
        assert_eq!(candidates[0].model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn profile_effective_routing_preserves_legacy_standalone_behavior() {
        let config: GahConfig = toml::from_str(
            r#"
[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"

[profiles.repo.routing]
pm_backend = "claude"
"#,
        )
        .unwrap();

        let profile = config.profiles.get("repo").unwrap();
        let effective = profile.effective_routing(&config.defaults);

        assert_eq!(effective.pm_backend.as_deref(), Some("claude"));
        assert_eq!(effective.default_backend, None);
    }

    #[test]
    fn profile_effective_routing_inherits_defaults_field_by_field() {
        let config: GahConfig = toml::from_str(
            r#"
[defaults.routing]
default_backend = "codex"
pm_candidates = [{ backend = "claude", model = "sonnet" }]
allow_review_fallback = true

[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"

[profiles.repo.routing]
improve_backend = "agy"
"#,
        )
        .unwrap();

        let profile = config.profiles.get("repo").unwrap();
        let effective = profile.effective_routing(&config.defaults);

        assert_eq!(effective.default_backend.as_deref(), Some("codex"));
        assert_eq!(effective.improve_backend.as_deref(), Some("agy"));
        assert_eq!(effective.pm_candidates.as_ref().map(Vec::len), Some(1));
        assert!(effective.allow_review_fallback);
    }
}
