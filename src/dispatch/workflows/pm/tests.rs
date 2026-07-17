use super::*;
use crate::dispatch::test_util::{init_repo, profile};
use std::collections::HashSet;
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

fn empty_ctx() -> super::PmPreflight {
    super::PmPreflight {
        rendered: String::new(),
        existing_tickets: vec![],
        open_mrs: String::new(),
        merged_mrs: String::new(),
    }
}

fn packet_json(key: &str, title: &str, depends_on: &str) -> String {
    format!(
        r#"{{"key":"{key}","title":"{title}","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{{"capability":"edit","min_tier":"standard"}},"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"depends_on":[{depends_on}],"uncovered_reason":"x"}}"#
    )
}

#[test]
fn apply_pm_plan_translates_fan_out_dependencies_to_assigned_identities() {
    // One prerequisite unlocks two dependents. Both dependents must resolve
    // the plan-local "base" key to base's actual assigned TICKET-NNN id, not
    // the opaque key.
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
    // One dependent requires two prerequisites; both must appear translated
    // in a single canonical "Blocked by" line.
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
    // "prereq" is judged a duplicate of existing work and is skipped, but
    // "dependent" still depends on it. The dependency must not silently
    // vanish -- it must surface as an auditable, unresolved note.
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

    let self_dep = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"human_required","recommended_routing":{"capability":"review","min_tier":"strong"},"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["k1"],"uncovered_reason":"x"}"#,
    );
    assert!(parse_pm_plan(&self_dep)
        .unwrap_err()
        .to_string()
        .contains("dependency cycle"));

    let two_packet_cycle = bounded_plan(
        r#"{"key":"k1","title":"A","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["k2"],"uncovered_reason":"x"},
           {"key":"k2","title":"B","objective":"o","task_class":"fix","difficulty":"easy","risk":"low","execution_disposition":"autonomous","recommended_routing":{"capability":"edit"},"acceptance_criteria":["c"],"verification_commands":["v"],"depends_on":["k1"],"uncovered_reason":"x"}"#,
    );
    assert!(parse_pm_plan(&two_packet_cycle)
        .unwrap_err()
        .to_string()
        .contains("dependency cycle"));
}
