//! Shared routing fixtures for domain-owned unit tests.

use super::RouteRequest;
use crate::availability::{Reason, Source};
use crate::config::{Defaults, Profile, RoutingPolicy, TaskRoutingRule};
use tempfile::TempDir;
use time::OffsetDateTime;

#[allow(clippy::too_many_arguments)]
pub(super) fn record_unavailable(
    state_path: &std::path::Path,
    backend: &str,
    model: Option<&str>,
    reason: Reason,
    source: Source,
    unavailable_until: Option<OffsetDateTime>,
    last_error_summary: Option<String>,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    crate::availability::record_unavailable(
        state_path,
        backend,
        model,
        None,
        reason,
        source,
        unavailable_until,
        last_error_summary,
        now,
    )
}

pub(super) fn record_available(
    state_path: &std::path::Path,
    backend: &str,
    model: Option<&str>,
    source: Source,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    crate::availability::record_available(state_path, backend, model, None, source, now)
}

pub(super) fn defaults() -> Defaults {
    Defaults {
        current_manager: None,
        artifact_root: String::new(),
        worktree_base: String::new(),
        llm_base_url: String::new(),
        llm_model_local: String::new(),
        llm_model_cloud: String::new(),
        routing: RoutingPolicy {
            default_backend: Some("codex".into()),
            weak_review_backend: Some("codex".into()),
            allow_review_fallback: true,
            ..RoutingPolicy::default()
        },
    }
}

pub(super) fn profile() -> Profile {
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
        validation_timeout_seconds: None,
        notify_command: None,
        routing: RoutingPolicy {
            pm_backend: Some("claude".into()),
            ..RoutingPolicy::default()
        },
        pacing: Default::default(),
        publishing: Default::default(),
    }
}

pub(super) fn path(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("availability.json")
}

pub(super) fn backend_available(name: &str) -> bool {
    matches!(
        name,
        "claude" | "codex" | "openhands" | "agy" | "agy-main" | "agy-second" | "opencode"
    )
}

pub(super) fn candidate_config(
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
) -> crate::config::CandidateConfig {
    crate::config::CandidateConfig {
        backend: backend.into(),
        model: model.map(str::to_string),
        quota_pool: quota_pool.map(str::to_string),
        priority: 0,
        included_in_quota: false,
        marginal_cost_usd: None,
        quota_usage_percent: None,
        quota_days_remaining: None,
        requires_approval: false,
    }
}

pub(super) fn implementation_request() -> RouteRequest<'static> {
    RouteRequest {
        last_failure_class: None,
        mode: "improve",
        requested_backend: "auto",
        requested_model: None,
        recommended_backend: Some("codex"),
        recommended_model: Some("strong"),
        session_id: None,
        usage_summary: None,
    }
}

pub(super) fn easy_docs_rule(candidates: Vec<crate::config::CandidateConfig>) -> TaskRoutingRule {
    TaskRoutingRule {
        modes: vec!["improve".into(), "fix".into()],
        task_classes: vec!["documentation".into()],
        difficulties: vec!["easy".into()],
        risks: vec!["low".into()],
        candidates,
    }
}
