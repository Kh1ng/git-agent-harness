use super::*;
use crate::dispatch::test_util::{init_repo, profile};
use std::fs;

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
fn pm_preflight_requires_manager_memory() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let err = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap_err();
    assert!(err.to_string().contains("PM mode requires manager memory"));
}

#[test]
fn pm_task_includes_preflight_context_and_rules() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
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

    let ctx = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap();
    let task = build_pm_plan_task(&profile(tmp.path()), &ctx, "Fix push auth").unwrap();
    assert!(task.contains("## Preflight Context"));
    assert!(task.contains("Remember open work."));
    assert!(task.contains("TICKET-002-auth.md: Fix push auth"));
    assert!(task.contains("Default action is to avoid creating new tickets."));
    assert!(task.contains("### Repo State"));
    assert!(task.contains("Current branch:"));
    assert!(task.contains("Recent commits:"));
}

#[test]
fn first_heading_skips_non_headings() {
    assert_eq!(
        first_markdown_heading("intro\n## Heading\n"),
        Some("Heading")
    );
}

#[test]
fn parse_pm_plan_extracts_json_from_log() {
    let plan =
        parse_pm_plan("noise\n{\"title\":\"T\",\"summary\":\"S\",\"tickets\":[]}\n").unwrap();
    assert_eq!(plan.title, "T");
}

#[test]
fn apply_pm_plan_skips_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    let ctx = super::PmPreflight {
        rendered: String::new(),
        existing_tickets: vec!["- TICKET-001-fix.md: Fix login".into()],
        open_mrs: String::new(),
        merged_mrs: String::new(),
    };
    let plan: PmPlan = serde_json::from_str(
        r#"{"title":"Plan","summary":"Summary","tickets":[
            {"key":"fix-login","title":"Fix login","objective":"dup","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"},
            {"key":"fix-auth","title":"Fix auth","objective":"new","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"}
        ]}"#,
    )
    .unwrap();

    let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].display().to_string().contains("fix-auth"));
}

#[test]
fn next_ticket_id_avoids_collision_with_manager_memory_reservation() {
    // TICKET-091 AC6/7: a ticket ID reserved only in manager memory
    // prose (no docs/tickets/ file yet) must not be reused -- this is
    // exactly how the TICKET-102/103/104 collisions happened.
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
fn parse_pm_plan_validates_bounded_packet() {
    let json = bounded_plan(
        r#"{"key":"k1","title":"Fix auth","objective":"Harden auth","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit","min_tier":"standard"},"affected_files":["src/auth.rs"],"acceptance_criteria":["auth rejects bad token"],"verification_commands":["pytest tests/test_auth.py"],"depends_on":[],"duplicate_evidence":[],"uncovered_reason":"No MR covers it."}"#,
    );
    let plan = parse_pm_plan(&json).unwrap();
    assert_eq!(plan.tickets.len(), 1);
    assert_eq!(plan.tickets[0].key, "k1");
    assert_eq!(plan.tickets[0].recommended_routing.capability, "edit");
}

#[test]
fn parse_pm_plan_rejects_duplicate_keys() {
    let json = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"},
           {"key":"k1","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"}"#,
    );
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(err.to_string().contains("duplicate work packet key"));
}

#[test]
fn parse_pm_plan_rejects_unknown_dependency() {
    let json = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["missing"],"uncovered_reason":"x"}"#,
    );
    let err = parse_pm_plan(&json).unwrap_err();
    assert!(err.to_string().contains("depends on unknown key"));
}

#[test]
fn parse_pm_plan_rejects_invalid_disposition_and_routing() {
    let bad_disposition = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"robot","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"}"#,
    );
    assert!(parse_pm_plan(&bad_disposition)
        .unwrap_err()
        .to_string()
        .contains("execution_disposition"));

    let bad_routing = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"magic"},"acceptance_criteria":["c"],"verification_commands":["v"],"uncovered_reason":"x"}"#,
    );
    assert!(parse_pm_plan(&bad_routing)
        .unwrap_err()
        .to_string()
        .contains("recommended_routing"));

    let ok_dep = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"human_required","recommended_routing":{"capability":"review","min_tier":"strong"},"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["k1"],"uncovered_reason":"x"}"#,
    );
    assert!(parse_pm_plan(&ok_dep).is_ok());
}
