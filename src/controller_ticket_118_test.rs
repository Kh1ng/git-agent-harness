#[cfg(test)]
mod ticket_118_tests {
    use super::*;
    use crate::status::StatusSnapshot;
    use crate::sync::SyncMrJson;
    use crate::config::{Profile, MergePolicy};
    use std::collections::HashMap;

    fn empty_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            profile: Profile {
                manager_wake_autonomy: Default::default(),
                prune_older_than_days: None,
                display_name: "test".to_string(),
                repo_id: "test".to_string(),
                repo: "test/repo".to_string(),
                provider: "github".to_string(),
                local_path: "/tmp/test".to_string(),
                artifact_root: "/tmp/test".to_string(),
                default_target_branch: "main".to_string(),
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
                routing: Default::default(),
                pacing: Default::default(),
                publishing: Default::default(),
                merge_policy: MergePolicy::Auto,
            },
            merge_requests: vec![],
            fix_attempt_counts: HashMap::new(),
            merge_attempt_counts: HashMap::new(),
            review_held_work_ids: vec![],
            ..Default::default()
        }
    }

    fn ci_failed_mr(branch: &str, work_id: &str) -> SyncMrJson {
        SyncMrJson {
            profile: Some("test".to_string()),
            branch: branch.to_string(),
            work_id: Some(work_id.to_string()),
            id: Some("123".to_string()),
            url: Some("https://github.com/test/repo/pull/123".to_string()),
            title: Some("Test PR".to_string()),
            state: Some("open".to_string()),
            draft: true,
            merge_status: Some("clean".to_string()),
            merged: false,
            merged_at: None,
            ci_passed: false,
            ci_pending: false,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            review_gate_reason: None,
            classification: "CI_FAILED".to_string(),
            recommended_action: crate::sync::RecommendedAction::ReuseBranch,
        }
    }

    #[test]
    fn ci_failed_draft_pr_triggers_fix_mr_action() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(ci_failed_mr("gah/test-branch", "TICKET-118"));
        
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, reason, .. } => {
                assert_eq!(branch, "gah/test-branch");
                assert!(reason.contains("reusing existing branch"));
                assert!(reason.contains("CI_FAILED"));
            }
            other => panic!("Expected FixMr action for CI_FAILED MR, got {:?}", other),
        }
    }

    #[test]
    fn ci_failed_fix_mr_respects_retry_cap() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("gah/test-branch".to_string(), 2);
        snapshot.merge_requests.push(ci_failed_mr("gah/test-branch", "TICKET-118"));
        
        let action = decide_next_action(&snapshot);
        // After retry cap is reached, should be a work-item block (no_op), not profile freeze
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn needs_fix_also_triggers_fix_mr_action() {
        let mut snapshot = empty_snapshot();
        let mut mr = ci_failed_mr("gah/test-branch", "TICKET-118");
        mr.classification = "NEEDS_FIX".to_string();
        snapshot.merge_requests.push(mr);
        
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, reason, .. } => {
                assert_eq!(branch, "gah/test-branch");
                assert!(reason.contains("reusing existing branch"));
                assert!(reason.contains("NEEDS_FIX"));
            }
            other => panic!("Expected FixMr action for NEEDS_FIX MR, got {:?}", other),
        }
    }

    #[test]
    fn fix_mr_action_uses_existing_branch_not_new() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(ci_failed_mr("gah/existing-branch", "TICKET-118"));
        
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, .. } => {
                // Should reuse the existing branch, not create a new one
                assert_eq!(branch, "gah/existing-branch");
            }
            other => panic!("Expected FixMr action, got {:?}", other),
        }
    }
}