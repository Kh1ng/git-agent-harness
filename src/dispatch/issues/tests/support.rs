use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
use std::collections::HashMap;
use std::path::Path;

pub(super) fn profile(local_path: &Path) -> Profile {
    Profile {
        delivery_mode: crate::config::DeliveryMode::default(),
        manager_wake_autonomy: crate::config::WakeAutonomy::default(),
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
        agy_print_timeout_seconds: HashMap::new(),
        agy_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds_by_model: HashMap::new(),
        max_concurrent_per_model: HashMap::new(),
        openhands_idle_timeout_seconds: None,
        vibe_idle_timeout_seconds: None,
        codex_idle_timeout_seconds: None,
        claude_idle_timeout_seconds: None,
        max_parallel_workers: None,
        max_open_managed_mrs: None,
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
        notify_command: None,
        routing: RoutingPolicy::default(),
        pacing: Default::default(),
        publishing: crate::config::PublishingPolicy {
            issue_intake_mode: crate::config::IssueIntakeMode::Legacy,
            ..Default::default()
        },
        prune_older_than_days: None,
    }
}

pub(super) fn ticket_cfg(root: &Path) -> GahConfig {
    GahConfig {
        context: Default::default(),
        defaults: Defaults {
            current_manager: None,
            artifact_root: root.to_string_lossy().into_owned(),
            worktree_base: root.to_string_lossy().into_owned(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing: RoutingPolicy::default(),
        },
        profiles: HashMap::new(),
    }
}
