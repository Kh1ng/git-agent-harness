use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

mod issue_intake;
pub use issue_intake::IssueIntakeMode;
mod publishing;
pub use publishing::PublishingPolicy;

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

/// How much a woken manager agent (see `Defaults::current_manager`) is
/// allowed to do on its own when a notify-worthy event fires (MR ready,
/// human required, review verdict, terminal dispatch failure).
/// Deliberately per-profile, not global -- an operator sprinting on one
/// project may want full autonomy while another project's operator wants
/// to decide every merge themselves.
///
/// * `Off` (default): no wake, `notify_command` behavior is unchanged.
/// * `ReviewOnly`: the woken agent reviews and posts findings, but must not
///   merge or take any other write action.
/// * `Full`: the woken agent may act on its own judgment (review and merge
///   if CI is green and review passed, investigate and fix a failure,
///   etc.) under the same standing authorization a human operator would
///   otherwise apply manually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WakeAutonomy {
    #[default]
    Off,
    ReviewOnly,
    Full,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GahConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
    #[serde(default)]
    pub context: crate::context::ContextConfig,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
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
    /// Which agent CLI ("claude" | "codex" | "hermes") is currently acting
    /// as the operator's manager across all profiles/projects. Read by the
    /// manager-wake feature (`Profile::manager_wake_autonomy`) to decide
    /// who to invoke when a notify-worthy event fires. Global, not
    /// per-profile -- "who's on call" is a cross-project fact, unlike
    /// autonomy bounds. `None`/unrecognized values mean no wake happens
    /// even if a profile has autonomy enabled.
    #[serde(default)]
    pub current_manager: Option<String>,
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

    /// Directory `manager_wake_autonomy` audit logs are written under, same
    /// override convention as `GAH_LEDGER_PATH`/`GAH_EVENTS_PATH`.
    pub fn manager_wake_log_dir(&self) -> PathBuf {
        if let Ok(path) = std::env::var("GAH_MANAGER_WAKE_LOG_DIR") {
            return PathBuf::from(path);
        }
        if !self.artifact_root.trim().is_empty() {
            return PathBuf::from(self.artifact_root.trim()).join("manager-wake-logs");
        }
        default_config_dir().join("manager-wake-logs")
    }
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
    /// How long OpenCode can go without a durable worktree change before GAH
    /// considers it stalled and kills it, in seconds. OpenCode's narration
    /// and malformed tool-call output deliberately do not reset this window:
    /// only repository progress does. This retains an activity-based guard
    /// rather than imposing a flat dispatch deadline. Defaults to 300s when
    /// unset.
    #[serde(default)]
    pub opencode_idle_timeout_seconds: Option<u64>,
    /// Per-model override for `opencode_idle_timeout_seconds`, keyed by the
    /// exact model name passed to `opencode --model` (e.g.
    /// "litellm-lan/qwen3.6:35b-a3b"). Some models routed through opencode
    /// are genuinely slow-but-working (a self-hosted litellm proxy) rather
    /// than hung, while others (a free-tier rate-limited model) hang with
    /// zero output and should be killed fast -- one flat idle timeout for
    /// every opencode model is too coarse. Falls back to the flat
    /// `opencode_idle_timeout_seconds` when no entry matches. Same pattern
    /// as `agy_print_timeout_seconds` below.
    #[serde(default)]
    pub opencode_idle_timeout_seconds_by_model: HashMap<String, u64>,
    /// Per-backend/model concurrency caps keyed by `"{backend}/{model}"`.
    /// Missing keys are unlimited. Enforcement is process-local; the profile
    /// lock guarantees one dispatching GAH process per profile.
    #[serde(default)]
    pub max_concurrent_per_model: HashMap<String, u32>,
    /// OpenHands idle-output timeout in seconds. Defaults to 300s; active
    /// worktree progress keeps a quiet process alive.
    #[serde(default)]
    pub openhands_idle_timeout_seconds: Option<u64>,
    /// How long Vibe's own log output can go quiet before GAH considers it
    /// stalled and kills it, in seconds. Same rationale and mechanism as
    /// `opencode_idle_timeout_seconds` -- added after a live dispatch hung
    /// for 15+ minutes with vibe as the backend and `run_vibe_with_executable`
    /// had zero supervision of any kind. Defaults to 300s when unset.
    #[serde(default)]
    pub vibe_idle_timeout_seconds: Option<u64>,
    /// How long Codex's own log output can go quiet before GAH considers it
    /// stalled and kills it, in seconds. Same rationale and mechanism as
    /// `opencode_idle_timeout_seconds`. Defaults to 300s when unset.
    #[serde(default)]
    pub codex_idle_timeout_seconds: Option<u64>,
    /// How long Claude Code's own log output can go quiet before GAH
    /// considers it stalled and kills it, in seconds. Same rationale and
    /// mechanism as `opencode_idle_timeout_seconds`. Defaults to 300s when
    /// unset.
    #[serde(default)]
    pub claude_idle_timeout_seconds: Option<u64>,
    /// How many tickets `gah loop` may execute concurrently for this profile.
    /// The native recurring loop owns this worker pool; the shell supervisor
    /// is only a compatibility launcher. Defaults to 1 when unset.
    #[serde(default)]
    pub max_parallel_workers: Option<u32>,
    /// Open managed PRs/MRs plus pre-publication dispatches allowed before
    /// implementation intake pauses. Defaults to `max_parallel_workers`.
    #[serde(default)]
    pub max_open_managed_mrs: Option<u32>,
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
    /// How much a woken manager agent is allowed to do on its own for this
    /// profile, when a notify-worthy event fires. See `WakeAutonomy` and
    /// `Defaults::current_manager`. Defaults to `Off` -- an operator opts a
    /// specific profile into this.
    #[serde(default)]
    pub manager_wake_autonomy: WakeAutonomy,
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
    /// Best-effort mechanical fixups run before validation on every attempt.
    /// Example: ["cargo fmt"].
    #[serde(default)]
    pub auto_fix_commands: Vec<String>,
    #[serde(default)]
    pub test_file_patterns: Vec<String>,
    /// Explicit case-insensitive markers for known baseline failures.
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
    /// Review idle timeout in seconds (default 300).
    #[serde(default)]
    pub review_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub review_hard_timeout_seconds: Option<u64>,
    /// Per-command validation timeout in seconds (default 300).
    #[serde(default)]
    pub validation_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub routing: RoutingPolicy,
    /// TICKET-128: per-profile policy for human-facing repository messaging.
    /// Independent of reviewer routing and merge authorization; defaults to
    /// allowing PR/MR text, commit messages, and issue comments.
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
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
    /// Paid/API-backed candidates can remain configured as terminal
    /// fallbacks without ever being selected autonomously. An operator must
    /// grant a work-item-scoped route approval before routing may select one.
    #[serde(default)]
    pub requires_approval: bool,
}

/// A deterministic implementation-routing override selected from trusted
/// ticket metadata. Empty match lists are wildcards; every non-empty list
/// must match. Rules are evaluated in declaration order, so the first match
/// wins. Priority defines fallback tiers; equal-priority candidates are
/// balanced from runtime usage, with declaration order as the final tie-break.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct TaskRoutingRule {
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub task_classes: Vec<String>,
    #[serde(default)]
    pub difficulties: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub candidates: Vec<CandidateConfig>,
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
    /// Issue #123 / TICKET-118-stabilization: ROUTINE_REVIEWER -- the single
    /// STRONG first-line reviewer (e.g. Mistral-Medium via vibe). Replaces the
    /// deprecated `strong_review_backend`/`strong_review_model` pair; when set
    /// it is the authority used for ordinary review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_reviewer: Option<CandidateConfig>,
    /// Issue #123: ESCALATORY_REVIEW -- an ORDERED LIST of advanced reviewers
    /// (Sonnet, Kimi, GLM, ...) used when routine review escalates. This is a
    /// list, not a single backend, which the old `weak_review_*` fields could
    /// not express. An escalatory reviewer is a more-capable model the pipeline
    /// escalates to and continues with (auto-merge eligible), distinct from the
    /// legacy `weak_review_*` safety-net that forced `human_required`.
    #[serde(default)]
    pub escalatory_reviewers: Vec<CandidateConfig>,
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
    pub pm_guidance_paths: Vec<String>,
    /// Ordered deterministic overrides for implementation work classified by
    /// trusted ticket metadata. Empty means no class-specific override.
    #[serde(default)]
    pub task_routing_rules: Vec<TaskRoutingRule>,
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
    /// Maximum completed routine reviews; defaults to `max_fix_attempts_per_mr + 1`.
    /// Each explicitly configured escalatory backend/model retains one bounded
    /// attempt beyond this cap before human escalation.
    #[serde(default)]
    pub max_review_cycles_per_ticket: Option<u32>,
    /// Maximum post-review repair dispatches for one MR before a human must
    /// intervene. A `NEEDS_FIX` verdict always gets this repair budget before
    /// it can become a human-required state. Unset defaults to two repairs.
    #[serde(default)]
    pub max_fix_attempts_per_mr: Option<u32>,
    /// Maximum paid/API-backed reviews for one work item. Unset defaults to
    /// three; quota-backed and local reviews do not consume this cap.
    #[serde(default)]
    pub max_paid_reviews_per_ticket: Option<u32>,
    /// Maximum genuine implementation failures before the controller gives
    /// up. This is deliberately separate from the post-review repair budget:
    /// a multi-model implementation ladder needs enough room to try each
    /// configured subscription candidate before human handoff. Defaults to 8.
    #[serde(default)]
    pub max_implementation_failures_per_ticket: Option<u32>,
    /// TICKET-127/Issue #124: per-repo merge policy gating what the
    /// controller does for a `READY_FOR_HUMAN` MR whose CI has been evaluated.
    /// `None` inherits the canonical/defaults policy (resolved to `Auto`).
    #[serde(default)]
    pub merge_policy: Option<MergePolicy>,
}

impl RoutingPolicy {
    pub fn merged_with_defaults(&self, defaults: &RoutingPolicy) -> RoutingPolicy {
        merge_routing_policy(defaults.clone(), self.clone())
    }

    /// Issue #123: resolve the effective ROUTINE_REVIEWER (STRONG tier).
    ///
    /// Prefers the new `routine_reviewer` field; falls back to the deprecated
    /// `strong_review_backend`/`strong_review_model` pair so existing configs
    /// keep working unchanged. Returns `None` when no routine reviewer is
    /// declared (caller decides whether that is a hard error or a warning).
    pub fn effective_routine_reviewer(&self) -> Option<CandidateConfig> {
        if let Some(r) = &self.routine_reviewer {
            return Some(r.clone());
        }
        match (&self.strong_review_backend, &self.strong_review_model) {
            (Some(b), m) => Some(CandidateConfig {
                backend: b.clone(),
                model: m.clone(),
                ..CandidateConfig::default()
            }),
            _ => None,
        }
    }

    /// Issue #123: resolve the effective ESCALATORY_REVIEW list (ordered).
    ///
    /// Prefers the new `escalatory_reviewers` list; falls back to the
    /// deprecated single `weak_review_backend`/`weak_review_model` entry so
    /// existing configs (which used the weak tier as a one-entry escalatory
    /// cascade) keep working. Returns an empty list when nothing is declared.
    pub fn effective_escalatory_reviewers(&self) -> Vec<CandidateConfig> {
        if !self.escalatory_reviewers.is_empty() {
            return self.escalatory_reviewers.clone();
        }
        match (&self.weak_review_backend, &self.weak_review_model) {
            (Some(b), m) => vec![CandidateConfig {
                backend: b.clone(),
                model: m.clone(),
                ..CandidateConfig::default()
            }],
            _ => vec![],
        }
    }

    #[allow(dead_code)] // enforced by dispatch review budget checks (#113)
    pub fn max_review_cycles_per_ticket(&self) -> u32 {
        self.max_review_cycles_per_ticket
            .unwrap_or_else(|| self.max_fix_attempts_per_mr().saturating_add(1))
    }

    pub fn max_fix_attempts_per_mr(&self) -> u32 {
        self.max_fix_attempts_per_mr.unwrap_or(2)
    }

    #[allow(dead_code)] // enforced by dispatch review budget checks (#113)
    pub fn max_paid_reviews_per_ticket(&self) -> u32 {
        self.max_paid_reviews_per_ticket.unwrap_or(3)
    }

    pub fn max_implementation_failures_per_ticket(&self) -> u32 {
        self.max_implementation_failures_per_ticket.unwrap_or(8)
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
        let configured = candidates.and_then(|list| {
            list.iter()
                .find(|c| c.backend == backend && c.model.as_deref() == model)
                .and_then(|c| c.quota_pool.as_deref())
        });
        crate::availability::resolve_candidate_quota_pool(backend, model, configured)
    }
}

impl Profile {
    pub fn effective_routing(&self, defaults: &Defaults) -> RoutingPolicy {
        self.routing.merged_with_defaults(&defaults.routing)
    }

    /// An explicit executable path override for `backend`, if this profile
    /// sets one. `resolve_backend_executable` (in `runner::resolve`) treats a
    /// `Some` return as a literal file path to check with `is_executable_path`
    /// -- this must ONLY ever return a real path override, never a marker
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
    /// this profile sets an `oh_profile`. Settings should call this instead of
    /// `configured_backend_path`; an earlier version made OpenHands
    /// unavailable for every explicit
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

    pub fn validation_timeout_seconds(&self) -> u64 {
        self.validation_timeout_seconds.unwrap_or(300).max(1)
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

    pub fn vibe_idle_timeout_seconds(&self) -> u64 {
        self.vibe_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn codex_idle_timeout_seconds(&self) -> u64 {
        self.codex_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn claude_idle_timeout_seconds(&self) -> u64 {
        self.claude_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn max_parallel_workers(&self) -> u32 {
        self.max_parallel_workers.unwrap_or(1).max(1)
    }

    pub fn max_open_managed_mrs(&self) -> u32 {
        self.max_open_managed_mrs
            .unwrap_or_else(|| self.max_parallel_workers())
            .max(1)
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

/// Canonicalizes `--backend` aliases that execute the same backend but were
/// being recorded under their literal alias string, producing duplicate
/// cards for one backend on the quota page (e.g. "openhands" and
/// "cloud-coder" both run OpenHands via `runner::backend_command_name`, but
/// only "openhands" was ever normalized there -- the raw CLI string was
/// still what got written to `requested_backend`/`effective_backend` and
/// from there into the ledger). Applied both where new dispatches are
/// routed (dispatch.rs) and when grouping the ledger for the quota page
/// (ledger/mod.rs), so it also merges pre-existing historical entries recorded
/// under the old alias rather than only preventing new duplicates.
/// Deliberately does NOT touch "auto": that backend's *effective* backend
/// is resolved dynamically per-attempt by `routing::decide`, not a fixed
/// alias, so it must pass through unchanged.
pub fn canonical_backend_name(name: &str) -> &str {
    if name == "cloud-coder" {
        "openhands"
    } else {
        name
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
    #[cfg(test)]
    {
        if let Some(path) = CANONICAL_CONFIG_TEST_OVERRIDE.with(|cell| cell.borrow().clone()) {
            return path;
        }
    }
    std::env::var("GAH_CANONICAL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_config_dir().join("canonical.toml"))
}

// Tests used to coordinate this via a process-global env var (GAH_CANONICAL_CONFIG)
// guarded by a mutex, but that only serialized the tests that *set* the var --
// any other test calling `load()` concurrently on a different thread could still
// read the env var mid-mutation. A thread-local override sidesteps the race
// entirely: cargo test gives each running test exclusive use of its own thread.
#[cfg(test)]
thread_local! {
    static CANONICAL_CONFIG_TEST_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_canonical_config_override(path: impl Into<PathBuf>) {
    CANONICAL_CONFIG_TEST_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(path.into()));
}

#[cfg(test)]
pub(crate) fn clear_canonical_config_override() {
    CANONICAL_CONFIG_TEST_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
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
    repo.routine_reviewer = repo.routine_reviewer.or(canonical.routine_reviewer);
    if repo.escalatory_reviewers.is_empty() {
        repo.escalatory_reviewers = canonical.escalatory_reviewers.clone();
    }
    repo.pm_candidates = repo.pm_candidates.or(canonical.pm_candidates);
    repo.improve_candidates = repo.improve_candidates.or(canonical.improve_candidates);
    if repo.pm_guidance_paths.is_empty() {
        repo.pm_guidance_paths = canonical.pm_guidance_paths;
    }
    if repo.task_routing_rules.is_empty() {
        repo.task_routing_rules = canonical.task_routing_rules.clone();
    }
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
    repo.max_review_cycles_per_ticket = repo
        .max_review_cycles_per_ticket
        .or(canonical.max_review_cycles_per_ticket);
    repo.max_fix_attempts_per_mr = repo
        .max_fix_attempts_per_mr
        .or(canonical.max_fix_attempts_per_mr);
    repo.max_paid_reviews_per_ticket = repo
        .max_paid_reviews_per_ticket
        .or(canonical.max_paid_reviews_per_ticket);
    repo.max_implementation_failures_per_ticket = repo
        .max_implementation_failures_per_ticket
        .or(canonical.max_implementation_failures_per_ticket);
    let mut capabilities = canonical.review_required_capabilities;
    capabilities.extend(repo.review_required_capabilities);
    repo.review_required_capabilities = capabilities;
    repo.merge_policy = repo.merge_policy.or(canonical.merge_policy);
    repo
}

pub fn check_profile_candidate_model_consistency(
    defaults: &Defaults,
    profile: &Profile,
) -> Result<(), Vec<String>> {
    let routing = profile.effective_routing(defaults);
    let mut candidates = Vec::new();
    if let Some(ref c) = routing.routine_reviewer {
        candidates.push(("routine_reviewer", c));
    }
    for c in &routing.escalatory_reviewers {
        candidates.push(("escalatory_reviewer", c));
    }
    if let Some(ref list) = routing.pm_candidates {
        for c in list {
            candidates.push(("pm_candidate", c));
        }
    }
    if let Some(ref list) = routing.improve_candidates {
        for c in list {
            candidates.push(("improve_candidate", c));
        }
    }
    for rule in &routing.task_routing_rules {
        for candidate in &rule.candidates {
            candidates.push(("task_routing_rule", candidate));
        }
    }
    if let Some(ref list) = routing.review_candidates {
        for c in list {
            candidates.push(("review_candidate", c));
        }
    }

    let mut errors = Vec::new();
    for (label, candidate) in candidates {
        let args = match candidate.backend.as_str() {
            "codex" => &profile.codex_args,
            "opencode" => &profile.opencode_args,
            "claude" => &profile.claude_args,
            _ => continue,
        };
        if let Some(pinned) =
            crate::runner::extract_model_from_backend_args(&candidate.backend, args)
        {
            if candidate.model.as_deref() != Some(&pinned) {
                errors.push(format!(
                    "candidate {} (backend '{}', label '{}') has mismatch with profile backend args model pin '{}'",
                    label,
                    candidate.backend,
                    candidate.model.as_deref().unwrap_or("None"),
                    pinned
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
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

    // Lint candidate model consistency
    for (name, profile) in &cfg.profiles {
        if let Err(mismatches) = check_profile_candidate_model_consistency(&cfg.defaults, profile) {
            for m in mismatches {
                eprintln!("WARNING: Profile '{}': {}", name, m);
            }
        }
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
        add_profile, canonical_backend_name, clear_canonical_config_override, get_profile_mut,
        load, load_canonical_routing, merge_routing_policy, remove_profile, save,
        set_canonical_config_override, CandidateConfig, GahConfig, Profile, RoutingPolicy,
    };

    /// Build a structurally complete `Profile` for unit tests in other modules
    /// (e.g. `notifications`). Mirrors the shape of `dispatch::tests::profile`
    /// so notification formatting tests can construct a real `Profile` without
    /// duplicating every field.
    #[cfg(test)]
    pub fn test_profile_for_notifications() -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
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
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            max_open_managed_mrs: None,
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            publishing: Default::default(),
            pacing: Default::default(),
        }
    }

    fn gitlab_profile(api_base: Option<&str>) -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
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
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            max_open_managed_mrs: None,
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            publishing: Default::default(),
            pacing: Default::default(),
        }
    }

    fn config_with_one_profile() -> GahConfig {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert("test".to_string(), test_profile_for_notifications());
        GahConfig {
            context: Default::default(),
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
    // oh_profile (e.g. "nous-hy3"), which resolve_backend_executable in
    // `runner::resolve` then treated as a literal executable file path --
    // that string is never a real file, so every explicit `--backend
    // openhands` dispatch on a profile with oh_profile set silently routed
    // away from openhands as if it were unavailable. configured_backend_path
    // must never return anything for openhands; is_backend_configured is the
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
                requires_approval: false,
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
                requires_approval: false,
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
        let tmp = tempfile::tempdir().unwrap();
        set_canonical_config_override(tmp.path().join("does-not-exist.toml"));
        let result = load_canonical_routing().unwrap();
        clear_canonical_config_override();
        assert!(result.is_none());
    }

    #[test]
    fn load_canonical_routing_fails_loudly_on_malformed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("canonical.toml");
        std::fs::write(&path, "not valid toml [[[").unwrap();
        set_canonical_config_override(&path);
        let result = load_canonical_routing();
        clear_canonical_config_override();
        assert!(result.is_err());
    }

    #[test]
    fn load_merges_canonical_into_repo_defaults_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join("canonical.toml");
        std::fs::write(
            &canonical_path,
            "[routing]\ndefault_backend = \"codex\"\nreview_backend = \"claude\"\n",
        )
        .unwrap();
        set_canonical_config_override(&canonical_path);

        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[defaults.routing]\ndefault_backend = \"agy\"\n",
        )
        .unwrap();

        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        clear_canonical_config_override();

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
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join("canonical.toml");
        std::fs::write(
            &canonical_path,
            "[routing]\nreview_backend = \"claude\"\nstrong_review_backend = \"claude\"\n",
        )
        .unwrap();
        set_canonical_config_override(&canonical_path);

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
        clear_canonical_config_override();

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
        // Issue #123: the shipped canonical example uses the new two-tier
        // reviewer scheme (routine_reviewer + escalatory_reviewers list).
        let routine = canonical.routing.effective_routine_reviewer().unwrap();
        assert_eq!(routine.backend, "claude");
        assert_eq!(routine.model.as_deref(), Some("sonnet"));
        let escalatory = canonical.routing.effective_escalatory_reviewers();
        assert_eq!(escalatory.len(), 1);
        assert_eq!(escalatory[0].backend, "codex");
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
                requires_approval: false,
            }]),
            task_routing_rules: vec![super::TaskRoutingRule {
                modes: vec!["improve".into()],
                task_classes: vec!["documentation".into()],
                difficulties: vec!["easy".into()],
                risks: vec!["low".into()],
                candidates: vec![super::CandidateConfig {
                    backend: "agy".into(),
                    model: Some("cheap".into()),
                    ..super::CandidateConfig::default()
                }],
            }],
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
        assert_eq!(merged.task_routing_rules.len(), 1);
        assert_eq!(
            merged.task_routing_rules[0].candidates[0].model.as_deref(),
            Some("cheap")
        );
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
                requires_approval: false,
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
                requires_approval: false,
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
    fn task_routing_rules_are_profile_overridable_as_one_ordered_policy() {
        let rule = |backend: &str| super::TaskRoutingRule {
            modes: vec!["improve".into()],
            task_classes: vec!["documentation".into()],
            difficulties: vec!["easy".into()],
            risks: vec!["low".into()],
            candidates: vec![super::CandidateConfig {
                backend: backend.into(),
                ..super::CandidateConfig::default()
            }],
        };
        let defaults = RoutingPolicy {
            task_routing_rules: vec![rule("agy")],
            ..RoutingPolicy::default()
        };
        let profile = RoutingPolicy {
            task_routing_rules: vec![rule("codex")],
            ..RoutingPolicy::default()
        };

        let merged = profile.merged_with_defaults(&defaults);
        assert_eq!(merged.task_routing_rules.len(), 1);
        assert_eq!(merged.task_routing_rules[0].candidates[0].backend, "codex");
    }

    #[test]
    fn review_budget_defaults_and_profile_override_are_deterministic() {
        let defaults = RoutingPolicy::default();
        assert_eq!(defaults.max_review_cycles_per_ticket(), 3);
        assert_eq!(defaults.max_fix_attempts_per_mr(), 2);
        assert_eq!(defaults.max_paid_reviews_per_ticket(), 3);
        assert_eq!(defaults.max_implementation_failures_per_ticket(), 8);

        let global = RoutingPolicy {
            max_review_cycles_per_ticket: Some(4),
            max_fix_attempts_per_mr: Some(3),
            max_paid_reviews_per_ticket: Some(5),
            max_implementation_failures_per_ticket: Some(10),
            ..RoutingPolicy::default()
        };
        let profile = RoutingPolicy {
            max_review_cycles_per_ticket: Some(1),
            max_fix_attempts_per_mr: Some(4),
            ..RoutingPolicy::default()
        };
        let effective = profile.merged_with_defaults(&global);
        assert_eq!(effective.max_review_cycles_per_ticket(), 1);
        assert_eq!(effective.max_fix_attempts_per_mr(), 4);
        assert_eq!(effective.max_paid_reviews_per_ticket(), 5);
        assert_eq!(effective.max_implementation_failures_per_ticket(), 10);

        let repair_budget_only = RoutingPolicy {
            max_fix_attempts_per_mr: Some(4),
            ..RoutingPolicy::default()
        };
        assert_eq!(repair_budget_only.max_review_cycles_per_ticket(), 5);
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

    #[test]
    fn canonical_backend_name_merges_cloud_coder_alias_into_openhands() {
        // Live-observed: --backend openhands and --backend cloud-coder both
        // run the identical OpenHands executable (runner::backend_command_name),
        // but nothing canonicalized the raw CLI string before it reached the
        // ledger/quota page, producing two separate cards for one backend.
        assert_eq!(canonical_backend_name("cloud-coder"), "openhands");
        assert_eq!(canonical_backend_name("openhands"), "openhands");
    }

    #[test]
    fn canonical_backend_name_leaves_other_backends_and_auto_untouched() {
        // "auto" must NOT be rewritten here: its effective backend is
        // resolved dynamically per-attempt by routing::decide, not a fixed
        // alias.
        for name in ["auto", "codex", "claude", "vibe", "opencode", "agy"] {
            assert_eq!(canonical_backend_name(name), name);
        }
    }
}
