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
            {"title":"Fix login","summary":"dup","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"},
            {"title":"Fix auth","summary":"new","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"}
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
