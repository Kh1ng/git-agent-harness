use super::*;
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config, init_repo, profile};
use crate::test_support::{ExecGuard, PathGuard};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn make_fake_bin(bin_dir: &Path, name: &str, body: &str) {
    let path = bin_dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
}

fn setup_fake_gh(bin_dir: &Path, issue_json: &str, open_pr_json: &str, merged_pr_json: &str) {
    let body = format!(
        "if [ \"$1\" = \"api\" ] && printf '%s' \"$*\" | grep -q '/issues?state=open'; then\n  printf '%s\\n' '{}'\nelif [ \"$1\" = \"api\" ] && printf '%s' \"$*\" | grep -q '/pulls?'; then\n  printf '%s\\n' '{}'\nelif [ \"$1\" = \"api\" ] && printf '%s' \"$*\" | grep -q 'search/issues'; then\n  printf '%s\\n' '{{\"incomplete_results\":false,\"items\":{}}}'\nelse\n  exit 97\nfi\n",
        issue_json.replace('\'', "'\\''"),
        open_pr_json.replace('\'', "'\\''"),
        merged_pr_json.replace('\'', "'\\''")
    );
    make_fake_bin(bin_dir, "gh", &body);
}

fn setup_fake_glab(bin_dir: &Path, issue_json: &str, mr_json: &str) {
    let body = format!(
        "if [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1\" = \"mr\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
        issue_json.replace('\'', "'\\''"),
        mr_json.replace('\'', "'\\''")
    );
    make_fake_bin(bin_dir, "glab", &body);
}

#[test]
fn ticket_summaries_include_filename_and_heading() {
    let tmp = tempfile::tempdir().unwrap();
    let tickets = tmp.path().join("docs/tickets");
    fs::create_dir_all(&tickets).unwrap();
    fs::write(tickets.join("TICKET-001-fix.md"), "# Fix login\nbody\n").unwrap();

    assert_eq!(
        collect_ticket_summaries(&tickets).unwrap(),
        vec!["- TICKET-001-fix.md: Fix login"]
    );
}

#[test]
fn pm_preflight_tolerates_missing_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    setup_fake_gh(&bin_dir, "[]", "[]", "[]");
    let _guard = PathGuard::set(&bin_dir);
    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let ctx = collect_pm_preflight(&cfg, &prof, tmp.path(), "target").unwrap();
    assert!(ctx.rendered.contains("### Source issue/story"));
    assert!(!ctx.rendered.contains("### Project Guidance"));
}

#[test]
fn pm_preflight_fails_closed_when_issue_snapshot_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    make_fake_bin(
        &bin_dir,
        "gh",
        "if [ \"$1\" = \"api\" ]; then exit 1; fi\nprintf '%s\\n' '[]'",
    );
    let _guard = PathGuard::set(&bin_dir);
    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let err = collect_pm_preflight(&cfg, &prof, tmp.path(), "target").unwrap_err();
    assert!(err.to_string().contains("complete open-issue snapshot"));
}

#[test]
fn pm_preflight_fails_closed_when_mr_snapshot_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    make_fake_bin(
        &bin_dir,
        "gh",
        "if printf '%s' \"$*\" | grep -q '/issues?state=open'; then printf '%s\\n' '[]'; exit 0; fi\nif [ \"$1\" = \"api\" ]; then exit 1; fi\nexit 97",
    );
    let _guard = PathGuard::set(&bin_dir);
    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let err = collect_pm_preflight(&cfg, &prof, tmp.path(), "target").unwrap_err();
    assert!(err.to_string().contains("complete PR/MR snapshot"));
}

#[test]
fn pm_preflight_fails_closed_when_github_open_pr_snapshot_hits_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    let prs = (0..100)
        .map(|number| {
            format!(
                r#"{{"number":{number},"title":"PR {number}","body":"","html_url":"https://example.com/pull/{number}","state":"open","draft":false,"head":{{"ref":"branch-{number}","sha":"sha-{number}"}},"updated_at":"2025-01-01T00:00:00Z"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    setup_fake_gh(&bin_dir, "[]", &format!("[{prs}]"), "[]");
    let _guard = PathGuard::set(&bin_dir);
    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let err = collect_pm_preflight(&cfg, &prof, tmp.path(), "target").unwrap_err();
    assert!(err.to_string().contains("complete PR/MR snapshot"));
    assert!(format!("{err:#}").contains("open-PR snapshot reached its cap (100)"));
}

#[test]
fn pm_preflight_collects_github_source_issues_and_mrs() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let issue_json = r##"[{"number":12,"title":"Fix auth","body":"body","labels":["exec:autonomous"],"author":{"login":"owner","type":"User","is_bot":false},"state":"OPEN"}]"##;
    let open_pr_json = r##"[
        {"number":12,"title":"#12 fix auth","body":"body","html_url":"https://example.com/pull/12","state":"open","draft":false,"head":{"ref":"gah/auth-fix","sha":"abc"},"updated_at":"2025-01-01T00:00:00Z"}
    ]"##;
    let merged_pr_json = r##"[
        {"number":99,"title":"#99 previous merge","body":"","html_url":"https://example.com/pull/99","updated_at":"2025-01-02T00:00:00Z","closed_at":"2025-01-02T00:00:00Z"}
    ]"##;

    setup_fake_gh(&bin_dir, issue_json, open_pr_json, merged_pr_json);
    let _guard = PathGuard::set(&bin_dir);

    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let preflight = collect_pm_preflight(&cfg, &prof, tmp.path(), "Add auth headers").unwrap();
    assert_eq!(preflight.open_issues_count, 1);
    assert_eq!(preflight.open_mr_count, 1);
    assert_eq!(preflight.merged_mr_count, 1);
    assert_eq!(preflight.source_issues.len(), 1);
    assert_eq!(preflight.open_mrs.len(), 1);
    assert_eq!(preflight.merged_mrs.len(), 1);
    assert!(preflight.open_mrs[0].contains("gah/auth-fix"));
    assert!(preflight.rendered.contains("body=body"));
}

#[test]
fn pm_preflight_collects_gitlab_source_issues_and_mrs() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let issue_json = r##"[{"iid":77,"title":"Fix parser","description":"Work ID: #77","labels":[],"author":{"username":"project_5_bot_77","bot":false},"state":"opened"}]"##;
    let mr_json = r##"[
        {"iid":11,"title":"#77 parser cleanup","description":"old","source_branch":"gah/parser-cleanup","web_url":"https://gitlab.example/group/project/-/merge_requests/11","labels":[],"state":"opened","draft":false,"detailed_merge_status":"can_be_merged","merge_status":"can_be_merged","updated_at":"2025-01-01T00:00:00Z","merged_at":null,"head_pipeline":{"status":"failed"}},
        {"iid":12,"title":"legacy refactor","description":"done","source_branch":"gah/legacy-refactor","web_url":"https://gitlab.example/group/project/-/merge_requests/12","labels":[],"state":"closed","draft":false,"detailed_merge_status":"can_be_merged","merge_status":"can_be_merged","updated_at":"2025-01-02T00:00:00Z","merged_at":"2025-01-03T00:00:00Z","head_pipeline":{"status":"success"}}
    ]"##;

    setup_fake_glab(&bin_dir, issue_json, mr_json);
    let _guard = PathGuard::set(&bin_dir);

    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "gitlab".into();
    prof.repo = "group/project".into();
    prof.provider_project_id = Some("5".into());
    prof.publishing.trusted_issue_human_authors = Some(vec!["project_5_bot_77".into()]);

    let preflight = collect_pm_preflight(&cfg, &prof, tmp.path(), "Refactor parser").unwrap();
    assert_eq!(preflight.open_issues_count, 1);
    assert_eq!(preflight.open_mr_count, 1);
    assert_eq!(preflight.merged_mr_count, 1);
    assert_eq!(preflight.source_issues.len(), 1);
    assert_eq!(preflight.open_mrs.len(), 1);
    assert_eq!(preflight.merged_mrs.len(), 1);
}

#[test]
fn pm_preflight_uses_configured_guidance_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    setup_fake_gh(&bin_dir, "[]", "[]", "[]");
    let _guard = PathGuard::set(&bin_dir);
    let guide = tmp.path().join("docs/PROJECT_GUIDANCE.md");
    fs::write(&guide, "# Guidance\nKeep it small.\n").unwrap();

    let routing = RoutingPolicy {
        pm_guidance_paths: vec!["docs/PROJECT_GUIDANCE.md".into()],
        ..Default::default()
    };
    let cfg = gah_config(routing);

    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();
    let preflight = collect_pm_preflight(&cfg, &prof, tmp.path(), "target").unwrap();
    assert!(preflight.rendered.contains("Keep it small."));
}

#[test]
fn pm_task_includes_preflight_context_and_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let _exec_guard = ExecGuard::new();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    init_repo(tmp.path());
    setup_fake_gh(&bin_dir, "[]", "[]", "[]");
    let _guard = PathGuard::set(&bin_dir);
    fs::create_dir_all(tmp.path().join("docs")).unwrap();
    fs::write(
        tmp.path().join("docs/MANAGER_MEMORY.md"),
        "# Memory\nRemember open work.\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("docs/tickets/TICKET-002-auth.md"),
        "# Fix push auth\n",
    )
    .unwrap();

    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();
    let ctx = collect_pm_preflight(&cfg, &prof, tmp.path(), "Fix push auth").unwrap();
    let task = build_pm_plan_task(&prof, &ctx, "Fix push auth").unwrap();
    assert!(task.contains("## Untrusted Preflight Context"));
    assert!(task.contains("## Target Request"));
    assert!(task.contains("TICKET-002-auth.md: Fix push auth"));
    assert!(task.contains("### Repo State"));
    assert!(task.contains("### Source issue/story"));
    assert!(task.contains("### Open native PR/MRs"));
    assert!(task.contains("### Recently merged PR/MRs"));
    assert!(task.contains("### Existing tickets"));
    assert!(task.contains("### Focus target"));
    assert!(task.contains("Current branch:"));
    assert!(task.contains("Proposed children must be atomic and non-overlapping"));
    assert!(task.contains("Default action: avoid creating new tickets unless there is a true gap"));
}

#[test]
fn first_heading_skips_non_headings() {
    assert_eq!(
        first_markdown_heading("intro\n## Heading\n"),
        Some("Heading")
    );
}

#[test]
fn parse_pm_plan_accepts_empty_plan() {
    let plan =
        parse_pm_plan("noise\n{\"title\":\"T\",\"summary\":\"S\",\"tickets\":[]}\n").unwrap();
    assert_eq!(plan.tickets.len(), 0);
}

#[test]
fn parse_pm_plan_parses_valid_json_from_log() {
    let plan = parse_pm_plan(
        "noise\n{\"title\":\"T\",\"summary\":\"S\",\"tickets\":[{\"key\":\"k1\",\"title\":\"Fix auth\",\"objective\":\"Harden auth\",\"task_class\":\"fix\",\"difficulty\":\"easy\",\"risk\":\"low\",\"execution_disposition\":\"autonomous\",\"recommended_routing\":{\"capability\":\"edit\",\"min_tier\":\"standard\"},\"affected_areas\":[\"auth\"],\"affected_files\":[\"src/auth.rs\"],\"acceptance_criteria\":[\"auth rejects bad token\"],\"verification_commands\":[\"pytest tests/test_auth.py\"],\"depends_on\":[],\"duplicate_evidence\":[],\"uncovered_reason\":\"No MR covers it.\"}]}\n",
    )
    .unwrap();
    assert_eq!(plan.tickets.len(), 1);
    assert_eq!(plan.tickets[0].key, "k1");
    assert_eq!(plan.tickets[0].summary, "Harden auth");
}

#[test]
fn apply_pm_plan_skips_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    let ctx = empty_ctx();
    let plan: PmPlan = serde_json::from_str(
        r##"{"title":"Plan","summary":"Summary","tickets":[
            {"key":"fix-login","title":"Fix login","objective":"dup","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["auth"],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"duplicate_evidence":["No matching native work item found."],"uncovered_reason":"x"},
            {"key":"fix-auth","title":"Fix auth","objective":"new","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["auth"],"affected_files":["b"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"duplicate_evidence":["No matching native work item found."],"uncovered_reason":"x"}
        ]}"##,
    )
    .unwrap();

    let mut ctx_dup = ctx;
    ctx_dup.existing_tickets = vec!["- TICKET-001-fix.md: Fix login".into()];
    let written = apply_pm_plan(repo, &ctx_dup, &plan).unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].display().to_string().contains("fix-auth"));
}

fn empty_ctx() -> super::PmPreflight {
    super::PmPreflight {
        rendered: String::new(),
        existing_tickets: vec![],
        source_issues: vec![],
        open_mrs: vec![],
        merged_mrs: vec![],
        open_issues_count: 0,
        open_mr_count: 0,
        merged_mr_count: 0,
    }
}

fn packet_json(key: &str, title: &str, depends_on: &str) -> String {
    format!(
        r#"{{"key":"{key}","title":"{title}","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{{"capability":"edit","min_tier":"standard"}},"affected_areas":["core"],"affected_files":["{key}.rs"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"depends_on":[{depends_on}],"duplicate_evidence":["No matching native work item found."],"uncovered_reason":"x"}}"#
    )
}

fn packet_json_with_objective(key: &str, title: &str, objective: &str) -> String {
    format!(
        r#"{{"key":"{key}","title":"{title}","objective":"{objective}","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{{"capability":"edit","min_tier":"standard"}},"affected_areas":["core"],"affected_files":["src/auth.rs"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}}"#
    )
}

fn packet_json_with_criteria(key: &str, title: &str, criteria: &str) -> String {
    format!(
        r#"{{"key":"{key}","title":"{title}","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{{"capability":"edit","min_tier":"standard"}},"affected_areas":["core"],"affected_files":["src/auth.rs"],"acceptance_criteria":["{criteria}"],"verification_commands":["pytest"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}}"#
    )
}

#[test]
fn apply_pm_plan_translates_fan_out_dependencies_to_assigned_identities() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    let ctx = empty_ctx();
    let tickets = [
        packet_json("base", "Base work", ""),
        packet_json("dep-a", "Dependent A", "\"base\""),
        packet_json("dep-b", "Dependent B", "\"base\""),
    ]
    .join(",");
    let plan: PmPlan = serde_json::from_str(&bounded_plan(&tickets)).expect("plan parses");

    let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
    assert_eq!(written.len(), 3);

    let base_path = written
        .iter()
        .find(|p| p.display().to_string().contains("base-work"))
        .unwrap();
    let base_id = fs::read_to_string(base_path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let base_ticket = base_id
        .trim_start_matches("# ")
        .split(':')
        .next()
        .unwrap()
        .to_string();

    for name in ["dependent-a", "dependent-b"] {
        let path = written
            .iter()
            .find(|p| p.display().to_string().contains(name))
            .unwrap();
        let body = fs::read_to_string(path).unwrap();
        assert!(
            body.contains(&format!("Blocked by: {base_ticket}")),
            "{name} body missing translated Blocked by line: {body}"
        );
        assert!(
            !body.contains("Blocked by: base"),
            "{name} leaked plan-local key"
        );
    }
}

#[test]
fn apply_pm_plan_translates_fan_in_dependencies_to_assigned_identities() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    let ctx = empty_ctx();
    let tickets = [
        packet_json("a", "Prereq A", ""),
        packet_json("b", "Prereq B", ""),
        packet_json("combined", "Combined dependent", "\"a\",\"b\""),
    ]
    .join(",");
    let plan: PmPlan = serde_json::from_str(&bounded_plan(&tickets)).expect("plan parses");

    let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
    assert_eq!(written.len(), 3);

    let combined_path = written
        .iter()
        .find(|p| p.display().to_string().contains("combined-dependent"))
        .unwrap();
    let body = fs::read_to_string(combined_path).unwrap();
    let blocked_by_line = body
        .lines()
        .find(|line| line.starts_with("Blocked by:"))
        .expect("combined dependent must have a Blocked by line");
    assert!(blocked_by_line.contains("TICKET-001"));
    assert!(blocked_by_line.contains("TICKET-002"));
}

#[test]
fn apply_pm_plan_retains_duplicate_skipped_dependency_instead_of_dropping_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    let mut ctx = empty_ctx();
    ctx.existing_tickets = vec!["- TICKET-001-prereq.md: Prereq work".into()];
    let tickets = [
        packet_json("prereq", "Prereq work", ""),
        packet_json("dependent", "Dependent work", "\"prereq\""),
    ]
    .join(",");
    let plan: PmPlan = serde_json::from_str(&bounded_plan(&tickets)).expect("plan parses");

    let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
    assert_eq!(
        written.len(),
        1,
        "prereq is a duplicate and must be skipped"
    );
    assert!(written[0].display().to_string().contains("dependent-work"));

    let body = fs::read_to_string(&written[0]).unwrap();
    assert!(
        !body.contains("Blocked by:"),
        "no assigned identity exists for a duplicate-skipped prerequisite: {body}"
    );
    assert!(
        body.contains("Unresolved Dependencies") && body.contains("prereq"),
        "duplicate-skipped dependency must not be silently dropped: {body}"
    );
}

#[test]
fn apply_pm_plan_assigns_unique_sequential_identities_without_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir).unwrap();
    fs::write(tickets_dir.join("TICKET-005-old.md"), "old").unwrap();
    let ctx = empty_ctx();
    let tickets = [
        packet_json("k1", "First", ""),
        packet_json("k2", "Second", ""),
        packet_json("k3", "Third", ""),
    ]
    .join(",");
    let plan: PmPlan = serde_json::from_str(&bounded_plan(&tickets)).expect("plan parses");

    let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
    let mut ids: Vec<String> = written
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .split('-')
                .nth(1)
                .unwrap()
                .to_string()
        })
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["006", "007", "008"]);
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 3, "no collisions");
}

#[test]
fn should_skip_ticket_across_context_entry_boundary() {
    let ticket: crate::models::PlannerWorkPacket = serde_json::from_str(
        r##"{"key":"dup","title":"Fix Auth","objective":"obj","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["auth"],"affected_files":["src/auth.rs"],"acceptance_criteria":["a"],"verification_commands":["pytest"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    )
    .unwrap();
    let ctx = super::PmPreflight {
        rendered: String::new(),
        existing_tickets: vec!["Fix".into(), "Auth".into()],
        source_issues: vec!["Legacy".into(), "Auth".into()],
        open_mrs: vec!["Issue".into(), "tracking".into()],
        merged_mrs: vec!["already".into(), "merged".into()],
        open_issues_count: 0,
        open_mr_count: 0,
        merged_mr_count: 0,
    };

    assert!(!super::should_skip_ticket(&ctx, &ticket));
}

#[test]
fn persist_pm_plan_artifact_written_before_provider_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    let docs = repo.join("docs");
    fs::write(&docs, b"blocking").unwrap();

    let preflight = super::PmPreflight {
        rendered: String::new(),
        existing_tickets: Vec::new(),
        source_issues: Vec::new(),
        open_mrs: Vec::new(),
        merged_mrs: Vec::new(),
        open_issues_count: 0,
        open_mr_count: 0,
        merged_mr_count: 0,
    };
    let plan: PmPlan = serde_json::from_str(
        &bounded_plan(
            r##"{"key":"k1","title":"Fix auth","objective":"Harden auth","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["auth"],"affected_files":["src/auth.rs"],"acceptance_criteria":["auth rejects bad token"],"verification_commands":["pytest tests/test_auth.py"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"No MR covers it."}"##,
        ),
    )
    .unwrap();

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let mut prof = profile(repo);
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();
    let err =
        super::persist_and_apply_pm_plan(&session_dir, repo, &prof, &preflight, "target", &plan)
            .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("not a directory"));
    assert!(
        session_dir.join("pm-plan-v1.json").exists(),
        "PM plan artifact must exist when apply fails"
    );
    assert!(
        !repo.join("docs/tickets").exists(),
        "provider mutation should not happen if artifact write succeeded and apply failed later"
    );
}

#[test]
fn next_ticket_id_avoids_collision_with_manager_memory_reservation() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir).unwrap();
    fs::write(tickets_dir.join("TICKET-005-old.md"), "old").unwrap();
    fs::write(
        repo.join("docs/MANAGER_MEMORY.md"),
        "## TICKET-042 -- reserved but not yet filed\n\nStatus: TODO\n",
    )
    .unwrap();

    let id = next_ticket_id(&tickets_dir, Some(&repo.join("docs/MANAGER_MEMORY.md"))).unwrap();
    assert_eq!(id, 43, "must skip past the memory-reserved TICKET-042");
}

fn bounded_plan(tickets_json: &str) -> String {
    format!(
        "{{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{}]}}",
        tickets_json
    )
}

#[test]
fn parse_pm_plan_rejects_oversized_plan_bytes() {
    let packet = packet_json_with_objective("k1", "Title", &"B".repeat(210_000));
    let json = bounded_plan(&packet);
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(
        err.to_string().contains("exceeded") || err.to_string().contains("too large"),
        "error message should indicate payload bound check: {err}"
    );
}

#[test]
fn parse_pm_plan_rejects_oversized_objective_field() {
    let packet = packet_json_with_objective("k1", "Title", &"B".repeat(5_000));
    let json = bounded_plan(&packet);
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(
        err.to_string().contains("objective too large"),
        "error message should indicate objective size check: {err}"
    );
}

#[test]
fn parse_pm_plan_rejects_oversized_criteria_field() {
    let packet = packet_json_with_criteria("k1", "Title", &"C".repeat(5_000));
    let json = bounded_plan(&packet);
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(
        err.to_string()
            .contains("oversized acceptance criteria entry"),
        "error message should indicate criteria size check: {err}"
    );
}

#[test]
fn parse_pm_plan_accepts_shared_area_labels_when_dependency_ordered_or_distinct_files() {
    let json_dep = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/a.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"},
           {"key":"k2","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/a.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["k1"],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&json_dep).is_ok());

    let json_distinct = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/a.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"},
           {"key":"k2","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/b.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&json_distinct).is_ok());
}

#[test]
fn parse_pm_plan_rejects_duplicate_keys() {
    let json = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["x"],"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"},
           {"key":"k1","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["y"],"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"}"##,
    );
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(err.to_string().contains("duplicate work packet key"));
}

#[test]
fn parse_pm_plan_rejects_unknown_dependency() {
    let json = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["x"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["missing"],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(err.to_string().contains("depends on unknown key"));
}

#[test]
fn parse_pm_plan_rejects_invalid_disposition_and_routing() {
    let bad_disposition = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"robot","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["x"],"acceptance_criteria":["c"],"verification_commands":["v"],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&bad_disposition)
        .unwrap_err()
        .to_string()
        .contains("execution_disposition"));

    let bad_routing = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"magic","min_tier":"standard"},"affected_areas":["core"],"affected_files":["x"],"acceptance_criteria":["c"],"verification_commands":["v"],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&bad_routing)
        .unwrap_err()
        .to_string()
        .contains("recommended_routing"));
}

#[test]
fn parse_pm_plan_rejects_empty_required_entries_and_uncovered_reason() {
    let empty_file = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":[""],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&empty_file)
        .unwrap_err()
        .to_string()
        .contains("empty affected files entry"));

    let missing_reason = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/a.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[]}"##,
    );
    assert!(parse_pm_plan(&missing_reason)
        .unwrap_err()
        .to_string()
        .contains("missing uncovered reason"));
}

#[test]
fn parse_pm_plan_rejects_unknown_schema_fields() {
    let unknown = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["src/a.rs"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x","invented":true}"##,
    );
    assert!(parse_pm_plan(&unknown)
        .unwrap_err()
        .to_string()
        .contains("unknown field"));
}

#[test]
fn parse_pm_plan_rejects_overlapping_packets_without_dependency() {
    let json = bounded_plan(
        r##"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["core"],"affected_files":["a","b"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"},
           {"key":"k2","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_areas":["docs","core"],"affected_files":["c","b"],"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"x"}"##,
    );
    assert!(parse_pm_plan(&json)
        .unwrap_err()
        .to_string()
        .contains("overlaps non-atomically"));
}

#[test]
fn build_pm_plan_task_rejects_oversized_context_instead_of_dropping_sections() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    let ctx = PmPreflight {
        rendered: "x".repeat(120_000),
        existing_tickets: Vec::new(),
        source_issues: Vec::new(),
        open_mrs: Vec::new(),
        merged_mrs: Vec::new(),
        open_issues_count: 0,
        open_mr_count: 0,
        merged_mr_count: 0,
    };
    let err = build_pm_plan_task(&prof, &ctx, "target").unwrap_err();
    assert!(err.to_string().contains("silently omit a section"));
}

#[test]
fn build_pm_plan_task_bounds_untrusted_target() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    let ctx = empty_ctx();
    let target = "y".repeat(10_000);

    let task = build_pm_plan_task(&prof, &ctx, &target).unwrap();
    assert!(!task.contains(&"y".repeat(5000)));
    assert!(task.len() < 45_000);
}

#[test]
fn pm_plan_artifact_deserializes_legacy_json_with_defaults() {
    let legacy_json = r#"{
        "profile": "real",
        "repo": "owner/repo",
        "target": "Fix bug",
        "ticket_count": 1,
        "plan": {
            "title": "Fix bug",
            "summary": "Summary",
            "tickets": [
                {
                    "key": "k1",
                    "title": "Fix bug",
                    "objective": "Objective",
                    "task_class": "fix",
                    "difficulty": "easy",
                    "risk": "low",
                    "execution_disposition": "autonomous",
                    "recommended_routing": {"capability": "edit", "min_tier": "standard"},
                    "affected_areas": ["core"],
                    "affected_files": ["src/main.rs"],
                    "acceptance_criteria": ["pass"],
                    "verification_commands": ["cargo test"],
                    "depends_on": [],
                    "duplicate_evidence": [],
                    "uncovered_reason": "none"
                }
            ]
        }
    }"#;

    let artifact: super::PmPlanArtifact =
        serde_json::from_str(legacy_json).expect("legacy artifact must deserialize cleanly");
    assert_eq!(artifact.schema_version, 1);
    assert_eq!(artifact.open_issue_count, 0);
    assert_eq!(artifact.open_mr_count, 0);
    assert_eq!(artifact.merged_mr_count, 0);
}

#[test]
fn pm_preflight_collects_non_gah_branch_prs_and_mrs() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let open_pr_json = r##"[
        {"number":10,"title":"Feature work","body":"body","html_url":"https://example.com/pull/10","state":"open","draft":false,"head":{"ref":"feat/my-feature","sha":"abc"},"updated_at":"2025-01-01T00:00:00Z"}
    ]"##;
    let merged_pr_json = r##"[
        {"number":11,"title":"Fix bug","body":"","html_url":"https://example.com/pull/11","updated_at":"2025-01-02T00:00:00Z","closed_at":"2025-01-02T00:00:00Z"}
    ]"##;

    setup_fake_gh(&bin_dir, "[]", open_pr_json, merged_pr_json);
    let _guard = PathGuard::set(&bin_dir);

    let cfg = gah_config(RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".into();
    prof.repo = "owner/repo".into();

    let preflight = collect_pm_preflight(&cfg, &prof, tmp.path(), "Add auth headers").unwrap();
    assert_eq!(preflight.open_mr_count, 1);
    assert_eq!(preflight.merged_mr_count, 1);
    assert_eq!(preflight.open_mrs.len(), 1);
    assert_eq!(preflight.merged_mrs.len(), 1);
    assert!(preflight.open_mrs[0].contains("feat/my-feature"));
    assert!(preflight.merged_mrs[0].contains("Fix bug"));
}

#[test]
fn parse_pm_plan_accepts_valid_plan_embedded_in_large_transcript() {
    let json_plan = r#"{"title":"T","summary":"S","tickets":[]}"#;
    let padding = "A".repeat(250_000);
    let log_text = format!("{padding}\n{json_plan}\n{padding}");
    let plan = parse_pm_plan(&log_text).unwrap();
    assert_eq!(plan.title, "T");
    assert_eq!(plan.summary, "S");
}
