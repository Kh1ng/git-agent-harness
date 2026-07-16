use super::*;
use crate::config::{Defaults, GahConfig, IssueIntakeMode, Profile, RoutingPolicy};
use crate::dispatch::scan_available_tickets;
use crate::ledger;
use crate::test_support::{ExecGuard, PathGuard};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn profile(local_path: &Path) -> Profile {
    Profile {
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

fn ticket_cfg(root: &Path) -> GahConfig {
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

#[test]
fn parses_ticket_metadata_for_routing() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket = tmp.path().join("TICKET-058-descriptive-mr-titles.md");
    fs::write(
            &ticket,
            "# TICKET-058: Descriptive MR Titles\n\nDifficulty: hard\nRisk: high\nRecommended backend: codex\nRecommended model: gpt-x\n\n## Affected Files\n- src/auth.rs\n\n## Verification Commands\n- `pytest tests/test_auth.py -x`\n",
        )
        .unwrap();
    let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
    assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-058"));
    assert_eq!(meta.title.as_deref(), Some("Descriptive MR Titles"));
    assert_eq!(meta.recommended_backend.as_deref(), Some("codex"));
    assert_eq!(meta.recommended_model.as_deref(), Some("gpt-x"));
    assert_eq!(meta.difficulty.as_deref(), Some("hard"));
    assert_eq!(meta.risk.as_deref(), Some("high"));
    assert_eq!(
        meta.verification_commands,
        vec!["pytest tests/test_auth.py -x"]
    );
}

#[test]
fn parses_structured_ticket_sections_into_typed_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket = tmp.path().join("TICKET-092-structured-work-metadata.md");
    fs::write(
        &ticket,
        "# TICKET-092: Structured work metadata\n\n\
Goal: Represent task metadata as typed structured fields rather than prompt parsing.\n\n\
Difficulty: medium\n\
Risk: medium\n\
Recommended backend: codex\n\
Recommended model: gpt-5.4\n\
Source: docs/tickets/TICKET-092-structured-work-metadata.md\n\n\
## Problem\n\
The parser should retain structured sections.\n\n\
## Acceptance Criteria\n\
- Define a single structured metadata type\n\
- Missing fields handled explicitly\n\n\
## Constraints\n\
- Do not require a new file format\n\
- No database\n\n\
## Affected Files\n\
- src/dispatch.rs\n\
- src/models.rs\n\n\
## Verification Commands\n\
- `cargo fmt --check`\n\
- `cargo test`\n",
    )
    .unwrap();

    let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
    assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-092"));
    assert_eq!(meta.work_id.as_deref(), Some("TICKET-092"));
    assert_eq!(meta.summary.as_deref(), Some("Structured work metadata"));
    assert_eq!(
        meta.problem.as_deref(),
        Some("The parser should retain structured sections.")
    );
    assert_eq!(
        meta.acceptance_criteria,
        vec![
            "Define a single structured metadata type",
            "Missing fields handled explicitly"
        ]
    );
    assert_eq!(
        meta.constraints,
        vec!["Do not require a new file format", "No database"]
    );
    assert_eq!(
        meta.affected_files,
        vec!["src/dispatch.rs", "src/models.rs"]
    );
    assert_eq!(
        meta.verification_commands,
        vec!["cargo fmt --check", "cargo test"]
    );
    assert_eq!(
        meta.source.as_deref(),
        Some("docs/tickets/TICKET-092-structured-work-metadata.md")
    );
}

#[test]
fn parses_ticket_metadata_preserves_colons_in_normal_headings() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket = tmp.path().join("TICKET-104-auth-hardening.md");
    fs::write(
        &ticket,
        "# Auth: reject empty token\n\nDifficulty: medium\nRisk: low\n",
    )
    .unwrap();

    let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
    assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-104"));
    assert_eq!(meta.title.as_deref(), Some("Auth: reject empty token"));
}

#[test]
fn parses_ticket_metadata_strips_ticket_prefix_from_heading_title() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket = tmp.path().join("TICKET-105-heading-title.md");
    fs::write(&ticket, "# TICKET-105: Keep title intact\n").unwrap();

    let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
    assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-105"));
    assert_eq!(meta.title.as_deref(), Some("Keep title intact"));
}

#[test]
fn parse_ticket_metadata_ignores_incidental_manager_memory_prose_mentions() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir).unwrap();

    fs::write(
        repo.join("docs/MANAGER_MEMORY.md"),
        "- **TICKET-114 is a serving-integrity control**\n\
             - **TICKET-110 before TICKET-112**\n",
    )
    .unwrap();

    let ticket_path = tickets_dir.join("TICKET-114-artifact-load-integrity.md");
    fs::write(
        &ticket_path,
        "# TICKET-114 — Artifact load integrity verification\n\nGoal: test\n",
    )
    .unwrap();

    let meta = parse_ticket_metadata(&ticket_path).unwrap().unwrap();
    assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-114"));
    assert_eq!(meta.work_id.as_deref(), Some("TICKET-114"));
    assert!(meta.is_authoritative);
}

#[test]
fn is_issue_number_reference_recognizes_plain_numbers() {
    assert!(is_issue_number_reference("42"));
    assert!(is_issue_number_reference("123"));
    assert!(!is_issue_number_reference("abc"));
    assert!(!is_issue_number_reference(""));
    assert!(!is_issue_number_reference("42abc"));
}

#[test]
fn is_issue_number_reference_recognizes_hash_numbers() {
    assert!(is_issue_number_reference("#42"));
    assert!(is_issue_number_reference("#123"));
    assert!(!is_issue_number_reference("#"));
    assert!(!is_issue_number_reference("#abc"));
    assert!(is_issue_number_reference(" #42 "));
}

#[test]
fn extract_issue_number_from_plain_number() {
    assert_eq!(extract_issue_number("42"), Some("42".to_string()));
    assert_eq!(extract_issue_number("123"), Some("123".to_string()));
    assert_eq!(extract_issue_number("abc"), None);
    assert_eq!(extract_issue_number(""), None);
}

#[test]
fn extract_issue_number_from_hash_number() {
    assert_eq!(extract_issue_number("#42"), Some("42".to_string()));
    assert_eq!(extract_issue_number("#123"), Some("123".to_string()));
    assert_eq!(extract_issue_number("#"), None);
    assert_eq!(extract_issue_number("#abc"), None);
}

#[test]
fn parse_ticket_metadata_from_issue_extracts_basic_fields() {
    let issue = IssueDetails {
            number: "42".to_string(),
            title: "TICKET-42: Fix the bug".to_string(),
            body:
                "## Problem\n\nSomething is broken\n\n## Acceptance Criteria\n\n- Fix the issue\n- Add tests"
                    .to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(meta.ticket_id.as_deref(), Some("#42"));
    assert_eq!(meta.work_id.as_deref(), Some("#42"));
    assert_eq!(meta.issue_number.as_deref(), Some("42"));
    assert_eq!(meta.title.as_deref(), Some("Fix the bug"));
    assert!(meta.is_authoritative);
    assert!(meta
        .acceptance_criteria
        .contains(&"Fix the issue".to_string()));
    assert!(meta.acceptance_criteria.contains(&"Add tests".to_string()));
}

#[test]
fn parse_ticket_metadata_from_issue_handles_metadata_fields() {
    let issue = IssueDetails {
            number: "42".to_string(),
            title: "Test Issue".to_string(),
            body: "Difficulty: High\nRisk: Medium\nRecommended backend: agy\nWork ID: TICKET-999\nGoal: Fix everything"
                .to_string(),
            labels: vec![],
            state: None,
        };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(meta.difficulty.as_deref(), Some("High"));
    assert_eq!(meta.risk.as_deref(), Some("Medium"));
    assert_eq!(meta.recommended_backend.as_deref(), Some("agy"));
    assert_eq!(meta.goal.as_deref(), Some("Fix everything"));
    assert_eq!(meta.title.as_deref(), Some("Test Issue"));
    assert_eq!(meta.work_id.as_deref(), Some("#42"));
}

#[test]
fn parse_ticket_metadata_from_issue_never_uses_goal_as_provider_title() {
    let issue = IssueDetails {
        number: "159".to_string(),
        title: "Migrate cache unification runbook".to_string(),
        body: format!("Goal: {}", "a long implementation goal ".repeat(20)),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(
        meta.title.as_deref(),
        Some("Migrate cache unification runbook")
    );
    assert!(meta.goal.as_deref().unwrap().len() > 255);
}

#[test]
fn parse_ticket_metadata_from_issue_folds_scope_and_invariants_378_style() {
    // Issue #405 / #378: `Scope` and `Invariants` headings were silently
    // dropped because only `Problem`/`Goal` and `Constraints` were
    // recognized.
    let issue = IssueDetails {
        number: "378".to_string(),
        title: "TICKET-378: Fix drift detection".to_string(),
        body: "## Scope\n\nDetect config drift across restarts\n\n\
                   ## Invariants\n\n- Never silently disable classification\n\
                   - Must fail closed on parse errors"
            .to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(
        meta.problem.as_deref(),
        Some("Detect config drift across restarts")
    );
    assert!(meta
        .constraints
        .contains(&"Never silently disable classification".to_string()));
    assert!(meta
        .constraints
        .contains(&"Must fail closed on parse errors".to_string()));
}

#[test]
fn parse_ticket_metadata_from_issue_folds_required_behavior_384_style() {
    // Issue #405 / #384: `Required Behavior` was silently dropped.
    let issue = IssueDetails {
        number: "384".to_string(),
        title: "TICKET-384: Eligibility gating".to_string(),
        body: "## Problem\n\nBad-ROI markets keep dispatching\n\n\
                   ## Required Behavior\n\n- Gate markets below the ROI floor\n\
                   - Preserve existing eligible markets"
            .to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(
        meta.problem.as_deref(),
        Some("Bad-ROI markets keep dispatching")
    );
    assert!(meta
        .constraints
        .contains(&"Gate markets below the ROI floor".to_string()));
    assert!(meta
        .constraints
        .contains(&"Preserve existing eligible markets".to_string()));
}

#[test]
fn parse_ticket_metadata_from_issue_scope_never_overrides_explicit_problem() {
    let issue = IssueDetails {
        number: "1".to_string(),
        title: "Test".to_string(),
        body: "## Problem\n\nThe real problem\n\n## Scope\n\nA scope note".to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(meta.problem.as_deref(), Some("The real problem"));
}

#[test]
fn parse_ticket_metadata_from_issue_scope_never_overrides_explicit_goal() {
    let issue = IssueDetails {
        number: "1".to_string(),
        title: "Test".to_string(),
        body: "## Goal\n\nShip the feature\n\n## Scope\n\nA scope note".to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(meta.problem, None);
    assert_eq!(meta.goal.as_deref(), Some("Ship the feature"));
}

#[test]
fn parse_ticket_metadata_from_issue_folds_prose_invariants_as_single_constraint() {
    // Not every Invariants/Required Behavior section is a bullet list --
    // prose content must not be silently dropped either.
    let issue = IssueDetails {
        number: "1".to_string(),
        title: "Test".to_string(),
        body: "## Problem\n\nSomething\n\n## Invariants\n\nMust remain fail-closed at all times."
            .to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert!(meta
        .constraints
        .contains(&"Must remain fail-closed at all times.".to_string()));
}

#[test]
fn parse_ticket_metadata_from_issue_accepts_move_only_and_verification_aliases() {
    let issue = IssueDetails {
        number: "425".to_string(),
        title: "Preserve ticket-425 heading shape".to_string(),
        body: "## Goal\n\nKeep the live task pack intact.\n\n## Move only\n\n- src/dispatch/claims.rs\n\n## Verification\n\n- `cargo test -p git-agent-harness --test dispatch`\n"
            .to_string(),
        labels: vec![],
        state: None,
    };

    let meta = parse_ticket_metadata_from_issue(&issue);
    assert_eq!(
        meta.goal.as_deref(),
        Some("Keep the live task pack intact.")
    );
    assert!(meta
        .affected_files
        .contains(&"src/dispatch/claims.rs".to_string()));
    assert!(meta
        .verification_commands
        .contains(&"cargo test -p git-agent-harness --test dispatch".to_string()));
}

#[test]
fn github_issue_intake_author_allowlist_is_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.repo = "Kh1ng/git-agent-harness".into();
    let owner = serde_json::json!({"author": {"login": "kh1ng", "type": "User", "is_bot": false}});
    let outsider =
        serde_json::json!({"author": {"login": "untrusted", "type": "User", "is_bot": false}});
    let missing = serde_json::json!({});

    assert!(github_issue_author_is_allowed(&prof, &owner));
    assert!(!github_issue_author_is_allowed(&prof, &outsider));
    assert!(!github_issue_author_is_allowed(&prof, &missing));

    prof.publishing.github_issue_author_allowlist = Some(vec!["teammate".into()]);
    let teammate =
        serde_json::json!({"author": {"login": "TEAMMATE", "type": "User", "is_bot": false}});
    assert!(github_issue_author_is_allowed(&prof, &teammate));
    assert!(!github_issue_author_is_allowed(&prof, &owner));

    prof.publishing.github_issue_author_allowlist = Some(vec![]);
    assert!(!github_issue_author_is_allowed(&prof, &teammate));
}

#[test]
fn canonical_autonomous_mode_rejects_unlabelled_github_issues() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\nRecommended model: Gemini 3.5 Flash (Medium)\n","labels":[],"author":{"login":"owner","type":"User","is_bot":false},"state":"OPEN"}]"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();
    prof.publishing.issue_intake_mode = IssueIntakeMode::CanonicalAutonomousOnly;

    let discovery = discover_open_issues(&prof);
    assert!(discovery.allowed.is_empty());
    assert_eq!(discovery.rejected.len(), 1);
    assert_eq!(
        discovery.rejected[0].reason_code,
        "canonical_autonomous_label_required"
    );

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert!(candidates.is_empty());
}

#[test]
fn trusted_gitlab_bot_authors_can_be_allowed_separately() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let issue_json = r#"[{"iid":77,"title":"TICKET-9: Legacy title must not become identity","description":"Work ID: TICKET-9\nRecommended backend: codex","labels":["exec:autonomous"],"author":{"id":46,"state":"active","username":"project_5_bot_deadbeef"},"state":"opened"}]"#;
    let glab_path = bin_dir.join("glab");
    fs::write(
            &glab_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&glab_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&glab_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "gitlab".to_string();
    prof.repo = "group/project".to_string();
    prof.provider_project_id = Some("5".into());
    prof.publishing.issue_intake_mode = IssueIntakeMode::CanonicalAutonomousOnly;
    prof.publishing.trusted_issue_bot_authors = Some(vec!["project_5_bot_deadbeef".into()]);

    let discovery = discover_open_issues(&prof);
    assert_eq!(discovery.allowed.len(), 1);
    assert!(discovery.rejected.is_empty());

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].work_id.as_deref(), Some("#77"));
}

#[test]
fn real_glab_human_shape_requires_the_gitlab_human_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.provider = "gitlab".into();
    prof.publishing.trusted_issue_human_authors = Some(vec!["teammate".into()]);
    let response = serde_json::json!({
        "author": {"id": 7, "state": "active", "username": "teammate"}
    });

    let author = parse_gitlab_author(&prof, &response).unwrap();
    assert_eq!(author.kind, IssueAuthorKind::Human);
    assert!(issue_author_is_trusted(&prof, &author));
}

#[test]
fn github_compatibility_allowlist_never_grants_gitlab_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.provider = "gitlab".into();
    prof.publishing.github_issue_author_allowlist = Some(vec!["teammate".into()]);
    let response = serde_json::json!({"author": {"username": "teammate"}});

    let author = parse_gitlab_author(&prof, &response).unwrap();
    assert!(!issue_author_is_trusted(&prof, &author));
}

#[test]
fn canonical_autonomous_intake_remains_opt_in_for_legacy_configs() {
    assert_eq!(
        crate::config::PublishingPolicy::default().issue_intake_mode,
        IssueIntakeMode::Legacy
    );
}

#[test]
fn conflicting_disposition_labels_resolve_to_owner_decision_regardless_of_order() {
    let labels = vec![
        "planning".into(),
        "blocked".into(),
        "exec:autonomous".into(),
        "exec:owner-decision".into(),
    ];

    assert_eq!(
        issue_disposition_from_labels(&labels),
        Some(IssueDisposition::OwnerDecision)
    );
}

#[test]
fn explicit_issue_fetch_requires_visible_override_for_unlabelled_discovery() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let issue_json = r#"{"number":42,"title":"Test issue","body":"Body","labels":[],"author":{"login":"owner","is_bot":false},"state":"OPEN"}"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"view\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();
    prof.publishing.issue_intake_mode = IssueIntakeMode::CanonicalAutonomousOnly;

    assert!(fetch_issue_details(&prof, "42", false).is_err());
    assert!(fetch_issue_details(&prof, "42", true).is_ok());
}

#[test]
fn scan_available_tickets_includes_open_github_issues() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\nRecommended model: Gemini 3.5 Flash (Medium)\n","labels":[],"author":{"login":"owner","type":"User","is_bot":false},"state":"OPEN"}]"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].ticket_path, "118");
    assert_eq!(candidates[0].work_id.as_deref(), Some("#118"));
    assert_eq!(candidates[0].recommended_backend.as_deref(), Some("agy"));
    assert_eq!(candidates[0].prior_attempt_count, 0);
    assert!(!candidates[0].has_active_mr);
}

#[test]
fn scan_available_tickets_uses_native_identity_for_gitlab_issues() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let issue_json = r#"[{"iid":77,"title":"TICKET-9: Legacy title must not become identity","description":"Work ID: TICKET-9\nRecommended backend: codex","labels":[],"author":{"username":"project-bot","bot":false},"state":"opened"}]"#;
    let glab_path = bin_dir.join("glab");
    fs::write(
            &glab_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&glab_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&glab_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "gitlab".to_string();
    prof.repo = "group/project".to_string();
    prof.publishing.trusted_issue_human_authors = Some(vec!["project-bot".into()]);

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].ticket_path, "77");
    assert_eq!(candidates[0].work_id.as_deref(), Some("#77"));
    assert_eq!(
        candidates[0].title.as_deref(),
        Some("Legacy title must not become identity")
    );
}

#[test]
fn scan_available_tickets_excludes_owner_decision_github_issues() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let issue_json = r#"[{"number":92,"title":"MS-5: Fleet ledger","body":"","labels":[{"name":"EXEC:OWNER-DECISION"}],"author":{"login":"owner"}}]"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);
    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );

    assert!(candidates.is_empty());
}

#[test]
fn scan_available_tickets_excludes_issue_already_archived_locally() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\n","labels":[],"author":{"login":"owner"}}]"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let closed_dir = tmp.path().join("docs/tickets/closed");
    fs::create_dir_all(&closed_dir).unwrap();
    fs::write(
        closed_dir.join("TICKET-101-fail-closed-version-drift.md"),
        "# TICKET-101: Fail closed\n\nGoal: test\n",
    )
    .unwrap();

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert!(
        candidates.is_empty(),
        "expected locally-archived TICKET-101 issue to be excluded, got {candidates:?}"
    );
}
