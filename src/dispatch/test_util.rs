use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
use std::path::Path;

pub(super) fn profile(local_path: &Path) -> Profile {
    Profile {
        manager_wake_autonomy: crate::config::WakeAutonomy::default(),
        prune_older_than_days: None,
        display_name: "Repo".into(),
        repo_id: "repo".into(),
        provider: "github".into(),
        repo: "owner/repo".into(),
        local_path: local_path.display().to_string(),
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
        notify_command: None,
        routing: RoutingPolicy::default(),
        pacing: Default::default(),
        publishing: Default::default(),
    }
}

pub(super) fn gah_config(routing: RoutingPolicy) -> GahConfig {
    GahConfig {
        context: Default::default(),
        defaults: Defaults {
            current_manager: None,
            artifact_root: String::new(),
            worktree_base: String::new(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing,
        },
        profiles: std::collections::HashMap::new(),
    }
}

/// Like `gah_config`, but with `artifact_root` pointed at a real tempdir so
/// ledger-backed tests have somewhere to persist their fixtures.
pub(super) fn gah_config_with_ledger(tmp: &Path, routing: RoutingPolicy) -> GahConfig {
    GahConfig {
        context: Default::default(),
        defaults: Defaults {
            current_manager: None,
            artifact_root: tmp.display().to_string(),
            worktree_base: String::new(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing,
        },
        profiles: std::collections::HashMap::new(),
    }
}
