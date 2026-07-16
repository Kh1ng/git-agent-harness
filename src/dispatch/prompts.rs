use super::issues::{
    extract_markdown_list_section, extract_markdown_requirement_items, extract_markdown_section,
    parse_ticket_metadata_from_issue, IssueDetails,
};
use super::text::utf8_safe_prefix;
use crate::config::{GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::models::Candidate;
use crate::models::CandidateArtifact;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Build the task prompt for the agent.
#[allow(clippy::too_many_arguments)]
pub(in crate::dispatch) fn enforce_context_budget(
    cfg: &GahConfig,
    _profile: &Profile,
    profile_name: &str,
    backend: &str,
    phase: &str,
    fresh_context: bool,
    prompt: &str,
    session_dir: &Path,
    run_id: Option<&str>,
    ledger: &mut LedgerEntry,
) -> Result<String> {
    let context_cfg = cfg.context.effective(profile_name, backend);
    let build = match crate::context::enforce(prompt, &context_cfg) {
        Ok(build) => build,
        Err(err) => {
            ledger.set_failure(
                crate::ledger::FailureClass::ContextLimitExceeded,
                crate::ledger::FailureStage::AgentRun,
            );
            ledger.context_phase = Some(phase.to_string());
            ledger.context_estimated_tokens_before = Some(crate::context::estimate_tokens(prompt));
            ledger.context_estimated_tokens_after = None;
            ledger.context_compacted = true;
            return Err(err);
        }
    };
    ledger.context_phase = Some(phase.to_string());
    ledger.context_estimated_tokens_before = Some(build.estimated_tokens_before_reduction);
    ledger.context_estimated_tokens_after = Some(build.estimated_tokens_after_reduction);
    ledger.context_compacted = build.compacted;
    let _ = fs::write(
        session_dir.join("context-built.json"),
        serde_json::to_vec_pretty(&build)?,
    );
    let details = serde_json::json!({
        "phase": phase,
        "backend": backend,
        "estimated_tokens_before_reduction": build.estimated_tokens_before_reduction,
        "estimated_tokens_after_reduction": build.estimated_tokens_after_reduction,
        "soft_limit_tokens": context_cfg.soft_limit_tokens,
        "hard_limit_tokens": context_cfg.hard_limit_tokens,
        "compacted": build.compacted,
        "fresh_context": fresh_context,
        "largest_sections": build.largest_sections,
        "sources": build.sources,
    });
    let _ = crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::ContextBuilt,
        Some(profile_name),
        ledger.work_id.as_deref(),
        run_id,
        details.to_string(),
    );
    Ok(build.prompt)
}

pub(super) const PROJECT_BRIEF_MAX_BYTES: usize = 10_000;
pub(super) const LIVE_TASK_FALLBACK_MAX_BYTES: usize = 12_000;
pub(super) const LIVE_TASK_TITLE_MAX_BYTES: usize = 1_024;
pub(super) const LIVE_TASK_LABELS_MAX_BYTES: usize = 2_048;
pub(super) const LIVE_TASK_PROBLEM_MAX_BYTES: usize = 4_096;
pub(super) const LIVE_TASK_ACCEPTANCE_MAX_BYTES: usize = 8_192;
pub(super) const LIVE_TASK_LIST_MAX_BYTES: usize = 4_096;
pub(super) const LIVE_TASK_LIST_ITEM_MAX_BYTES: usize = 1_024;

pub(super) fn build_task(
    profile: &Profile,
    wt: &Path,
    mode: &str,
    target: &str,
    issue_details: Option<&IssueDetails>,
) -> String {
    if let Some(issue) = issue_details {
        return build_task_with_issue(profile, wt, mode, issue);
    }
    // Try to load as candidate artifact
    if !target.is_empty() {
        let p = Path::new(target);
        if p.extension().is_some_and(|e| e == "json") && p.exists() {
            if let Ok(text) = fs::read_to_string(p) {
                if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                    if let Some(candidate) = artifact.candidates.first() {
                        return format_candidate_task(profile, wt, mode, candidate);
                    }
                }
            }
        }
    }

    let instruction = match mode {
        "fix" => {
            "Fix the specific issue described in the Focus section below.\n\
             Run the relevant tests to confirm the fix. All tests in the test suite must pass.\n\
             Do not push or create MRs."
        }
        "experiment" => {
            "This is a research/experiment task. Write scripts, generate Jupyter notebooks, \
             CSV exports, plots, or markdown reports directly in the working directory.\n\
             Do not worry about breaking unrelated tests. Prioritize producing observable \
             output files (*.ipynb, *.html, *.csv, *.png, *.md) over clean commits.\n\
             Do not push or create MRs."
        }
        _ if !target.is_empty() => {
            "Implement ONLY the specific ticket described in the Focus section below. \
             Ignore any other backlog items, priorities, or tickets mentioned in background \
             context -- those are not additional work to pick up.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
        _ => {
            "Select and implement the highest-priority improvement from the backlog, \
             recent CI failures, or test gaps.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
    };

    let mut task = format!(
        "Repository: {} ({})\nWorking directory: {}\nTarget branch: {}\n\n{}\n",
        profile.display_name,
        profile.repo,
        wt.display(),
        profile.default_target_branch,
        instruction,
    );

    append_project_brief(&mut task, profile);

    if !target.is_empty() {
        task.push_str(&format!("\n## Focus\n\n{}\n", target));
    }
    task
}

/// Build task with issue details for the Focus section
fn build_task_with_issue(profile: &Profile, wt: &Path, mode: &str, issue: &IssueDetails) -> String {
    let instruction = match mode {
        "fix" => {
            "Fix the specific issue described in the Focus section below.\n\
             Run the relevant tests to confirm the fix. All tests in the test suite must pass.\n\
             Do not push or create MRs."
        }
        "experiment" => {
            "This is a research/experiment task. Write scripts, generate Jupyter notebooks, \
             CSV exports, plots, or markdown reports directly in the working directory.\n\
             Do not worry about breaking unrelated tests. Prioritize producing observable \
             output files (*.ipynb, *.html, *.csv, *.png, *.md) over clean commits.\n\
             Do not push or create MRs."
        }
        _ => {
            "Implement ONLY the specific ticket described in the Focus section below. \
             Ignore any other backlog items, priorities, or tickets mentioned in background \
             context -- those are not additional work to pick up.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
    };

    let mut task = format!(
        "Repository: {} ({})\nWorking directory: {}\nTarget branch: {}\n\n{}\n",
        profile.display_name,
        profile.repo,
        wt.display(),
        profile.default_target_branch,
        instruction,
    );

    append_project_brief(&mut task, profile);
    append_live_task_pack(&mut task, issue);

    task.push_str(&format!(
        "\n## Focus\n\n{}\n",
        format_issue_focus_reference(issue)
    ));
    task
}

/// Worker prompts deliberately use a concise, committed project brief rather
/// than the live manager ledger. MANAGER_MEMORY remains PM-only operational
/// state: injecting it into every worker made stale status and retry history
/// compete with the ticket being executed.
fn append_project_brief(task: &mut String, profile: &Profile) {
    let brief_path = Path::new(&profile.local_path).join("docs/PROJECT_BRIEF.md");
    let Ok(brief) = fs::read_to_string(brief_path) else {
        return;
    };
    task.push_str("\n## Project Brief\n\n");
    append_bounded_text(task, &brief, PROJECT_BRIEF_MAX_BYTES, "Project brief");
}

/// Build a bounded, task-specific packet from structured issue metadata.
/// The free-form body is used only as a capped fallback when the issue did
/// not provide a structured problem, acceptance criteria, or constraints.
fn append_live_task_pack(task: &mut String, issue: &IssueDetails) {
    let meta = parse_ticket_metadata_from_issue(issue);
    let sections = extract_issue_sections(&issue.body);
    task.push_str("\n## Live Task Pack\n\n");
    task.push_str(&format!("Work item: #{} — ", issue.number));
    append_bounded_text(
        task,
        &indent_untrusted_text(&issue.title),
        LIVE_TASK_TITLE_MAX_BYTES,
        "Work item title",
    );
    if !issue.labels.is_empty() {
        task.push_str("Labels: ");
        append_bounded_text(
            task,
            &indent_untrusted_text(&issue.labels.join(", ")),
            LIVE_TASK_LABELS_MAX_BYTES,
            "Labels",
        );
    }
    if let Some(problem) = meta.problem.as_deref().or(meta.goal.as_deref()) {
        task.push_str("\n### Problem\n\n");
        append_bounded_text(
            task,
            &indent_untrusted_text(problem),
            LIVE_TASK_PROBLEM_MAX_BYTES,
            "Problem",
        );
    }
    append_task_pack_list(
        task,
        "Acceptance Criteria",
        &meta.acceptance_criteria,
        LIVE_TASK_ACCEPTANCE_MAX_BYTES,
    );
    append_task_pack_list(
        task,
        "Constraints",
        &meta.constraints,
        LIVE_TASK_LIST_MAX_BYTES,
    );
    append_task_pack_list(
        task,
        "Affected Files",
        &meta.affected_files,
        LIVE_TASK_LIST_MAX_BYTES,
    );
    if !meta.verification_commands.is_empty() {
        task.push_str("\n### Verification Commands\n\n");
        append_task_pack_list_items(
            task,
            &meta.verification_commands,
            LIVE_TASK_LIST_MAX_BYTES,
            true,
        );
    }
    append_unrecognized_issue_sections(task, &sections);
    if issue_has_no_structured_body(issue) {
        task.push_str("\n### Issue Description\n\n");
        append_bounded_text(
            task,
            &indent_untrusted_text(&issue.body),
            LIVE_TASK_FALLBACK_MAX_BYTES,
            "Issue description",
        );
    }
}

#[derive(Debug, Clone)]
struct IssueSection {
    heading: String,
    body: String,
}

fn extract_issue_sections(body: &str) -> Vec<IssueSection> {
    let mut sections = Vec::new();
    let mut heading = None;
    let mut body_lines = Vec::new();

    for raw_line in body.lines() {
        let line = raw_line.trim_start();
        if let Some(current_heading) = line.strip_prefix("## ") {
            let current_body = body_lines.join("\n").trim().to_string();
            if let Some(name) = heading.take() {
                if !current_body.is_empty() {
                    sections.push(IssueSection {
                        heading: name,
                        body: current_body,
                    });
                }
            }
            heading = Some(current_heading.trim().to_string());
            body_lines.clear();
            continue;
        }

        if heading.is_some() {
            body_lines.push(raw_line);
        }
    }

    let current_body = body_lines.join("\n").trim().to_string();
    if let Some(name) = heading {
        if !current_body.is_empty() {
            sections.push(IssueSection {
                heading: name,
                body: current_body,
            });
        }
    }

    sections
}

fn normalize_issue_heading(heading: &str) -> String {
    heading
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_live_task_pack_supported_section(heading: &str) -> bool {
    matches!(
        normalize_issue_heading(heading).as_str(),
        "problem"
            | "background"
            | "description"
            | "goal"
            | "scope"
            | "acceptance criteria"
            | "constraints"
            | "invariants"
            | "required behavior"
            | "verification commands"
            | "verification"
            | "affected files"
            | "move only"
            | "source"
    )
}

fn append_unrecognized_issue_sections(task: &mut String, sections: &[IssueSection]) {
    let mut started = false;
    for section in sections {
        if is_live_task_pack_supported_section(&section.heading) {
            continue;
        }

        if !started {
            task.push_str("\n### Additional Issue Sections\n\n");
            started = true;
        }

        task.push_str(&format!("  ## {}\n", section.heading));
        append_bounded_text(
            task,
            &indent_untrusted_text(&section.body),
            LIVE_TASK_LIST_MAX_BYTES,
            "Additional issue section",
        );
    }
}

fn append_task_pack_list(task: &mut String, heading: &str, entries: &[String], max_bytes: usize) {
    if entries.is_empty() {
        return;
    }
    task.push_str(&format!("\n### {heading}\n\n"));
    append_task_pack_list_items(task, entries, max_bytes, false);
}

fn append_task_pack_list_items(
    task: &mut String,
    entries: &[String],
    max_bytes: usize,
    code: bool,
) {
    let start = task.len();
    let mut truncated = false;
    for entry in entries {
        if task.len().saturating_sub(start) >= max_bytes {
            truncated = true;
            break;
        }
        let value = indent_untrusted_text(utf8_safe_prefix(entry, LIVE_TASK_LIST_ITEM_MAX_BYTES));
        let line = if code {
            format!("- `{value}`\n")
        } else {
            format!("- {value}\n")
        };
        if task.len().saturating_sub(start) + line.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(task.len().saturating_sub(start));
            if remaining > 3 {
                task.push_str(utf8_safe_prefix(&line, remaining));
            }
            truncated = true;
            break;
        }
        if entry.len() > LIVE_TASK_LIST_ITEM_MAX_BYTES {
            truncated = true;
        }
        task.push_str(&line);
    }
    if truncated {
        task.push_str(&format!(
            "[List truncated at {max_bytes} bytes; retrieve the issue for remaining detail.]\n"
        ));
    }
}

fn issue_has_no_structured_body(issue: &IssueDetails) -> bool {
    !issue
        .body
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with("## "))
        && extract_markdown_section(&issue.body, "Problem").is_none()
        && extract_markdown_section(&issue.body, "Background").is_none()
        && extract_markdown_section(&issue.body, "Description").is_none()
        && extract_markdown_section(&issue.body, "Goal").is_none()
        && extract_markdown_section(&issue.body, "Scope").is_none()
        && extract_markdown_list_section(&issue.body, "Acceptance Criteria").is_empty()
        && extract_markdown_list_section(&issue.body, "Constraints").is_empty()
        && extract_markdown_requirement_items(&issue.body, "Invariants").is_empty()
        && extract_markdown_requirement_items(&issue.body, "Required Behavior").is_empty()
}

fn append_bounded_text(task: &mut String, text: &str, max_bytes: usize, label: &str) {
    let truncated = text.len() > max_bytes;
    task.push_str(utf8_safe_prefix(text, max_bytes));
    if truncated {
        task.push_str(&format!(
            "\n\n[{label} truncated at {max_bytes} bytes; retrieve only relevant source material for more detail.]\n"
        ));
    } else if !text.ends_with('\n') {
        task.push('\n');
    }
}

pub(super) fn indent_untrusted_text(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_candidate_task(
    profile: &Profile,
    _wt: &Path,
    mode: &str,
    c: &Candidate,
) -> String {
    let mut out = format!(
        "# Task: {}\n\n\
         Repository: {} ({})\n\
         Local path: {}\n\
         Target branch: {}\n\n",
        c.candidate_id,
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
    );

    if !c.evidence.is_empty() {
        out.push_str("## Context\n");
        for e in &c.evidence {
            out.push_str(&format!("- {}\n", indent_untrusted_text(e)));
        }
        out.push('\n');
    }

    if !c.affected_files.is_empty() {
        out.push_str("## Files likely involved\n");
        for f in &c.affected_files {
            out.push_str(&format!("- {}\n", indent_untrusted_text(f)));
        }
        out.push('\n');
    }

    if !c.acceptance_criteria.is_empty() {
        out.push_str("## Acceptance criteria\n");
        for (i, ac) in c.acceptance_criteria.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, indent_untrusted_text(ac)));
        }
        out.push('\n');
    }

    if !c.verification.is_empty() {
        out.push_str("## Verification steps\n");
        for (i, v) in c.verification.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, indent_untrusted_text(v)));
        }
        out.push('\n');
    }

    append_project_brief(&mut out, profile);

    let closing = match mode {
        "fix" => {
            "Fix the code to satisfy every acceptance criterion above.\n\
             Run the verification steps to confirm. All tests must pass. Do not push or create MRs.\n"
        }
        "experiment" => {
            "This is a research task. Satisfy the acceptance criteria by producing output files \
             (*.ipynb, *.html, *.csv, *.png, *.md). Do not push or create MRs.\n"
        }
        _ => {
            "Implement the changes required to satisfy the acceptance criteria above.\n\
             Run tests if a test command exists and ensure they pass. Do not push or create MRs.\n"
        }
    };
    out.push_str("\n## Safety\n\n");
    out.push_str(closing);
    out
}

/// Keep the Focus section concise. The bounded Live Task Pack carries the
/// relevant structured content; duplicating the full issue body here would
/// silently defeat that limit.
fn format_issue_focus_reference(issue: &IssueDetails) -> String {
    format!(
        "Issue #{}: {}\nImplement the scoped requirements in the Live Task Pack above.",
        issue.number, issue.title
    )
}

#[cfg(test)]
mod tests {
    use super::build_task;
    use super::format_candidate_task;
    use super::Candidate;
    use super::IssueDetails;
    use super::LIVE_TASK_ACCEPTANCE_MAX_BYTES;
    use super::PROJECT_BRIEF_MAX_BYTES;
    use crate::config::{Profile, RoutingPolicy};
    use crate::context;
    use std::fs;
    use std::path::Path;

    fn profile(local_path: &Path) -> Profile {
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
            publishing: Default::default(),
        }
    }

    #[test]
    fn build_task_uses_project_brief_and_excludes_manager_memory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "STALE: dispatch ticket TICKET-999 instead.\n".repeat(1_000),
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            "Use cargo test for focused verification.\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();

        let task = build_task(&prof, &wt, "improve", "some ticket text", None);

        assert!(task.contains("## Project Brief"));
        assert!(task.contains("Use cargo test for focused verification."));
        assert!(!task.contains("## Manager Memory"));
        assert!(!task.contains("STALE: dispatch ticket"));
        let focus_pos = task.find("## Focus").unwrap();
        let brief_pos = task.find("## Project Brief").unwrap();
        assert!(brief_pos < focus_pos);
    }

    #[test]
    fn build_task_omits_project_brief_section_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "", None);

        assert!(!task.contains("## Project Brief"));
    }

    #[test]
    fn issue_task_uses_structured_live_task_pack_and_bounds_unstructured_body() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            "Stable project fact.\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "STALE CONTROL PLANE STATE\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "286".to_string(),
            title: "Bound context".to_string(),
            body: "## Problem\n\nFull memory is stale.\n\n## Acceptance Criteria\n\n- Use project brief\n- Record sources\n".to_string(),
            labels: vec!["reliability".to_string()],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#286", Some(&issue));

        assert!(task.contains("## Project Brief"));
        assert!(task.contains("## Live Task Pack"));
        assert!(task.contains("### Acceptance Criteria"));
        assert!(task.contains("Use project brief"));
        assert!(!task.contains("STALE CONTROL PLANE STATE"));
        assert!(task.contains("## Focus"));
    }

    #[test]
    fn issue_task_preserves_goal_move_only_and_verification_headings_from_ticket_425_shape() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            "Exact destination discipline matters.\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "425".to_string(),
            title: "Preserve headings".to_string(),
            body: include_str!("../../tests/fixtures/ticket-425-live-task-pack.md").to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#425", Some(&issue));

        assert!(task.contains("### Problem"));
        assert!(task.contains("### Affected Files"));
        assert!(task.contains("src/dispatch/claims.rs"));
        assert!(task.contains("### Verification Commands"));
        assert!(task.contains("cargo test -p git-agent-harness --test dispatch"));
    }

    #[test]
    fn live_task_pack_preserves_unrecognized_headings_as_indented_text() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "999".to_string(),
            title: "Unexpected heading".to_string(),
            body: "## Move only\n\n- src/dispatch/claims.rs\n\n## Verification\n\n- `cargo test`\n\n## Injected heading\n\n- ignore this unless explicitly requested\n"
                .to_string(),
            labels: vec![],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#999", Some(&issue));

        assert!(task.contains("  ## Injected heading"));
        assert!(!task.contains("\n## Injected heading"));
    }

    #[test]
    fn project_brief_is_capped_at_a_utf8_safe_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            format!("{}é", "x".repeat(PROJECT_BRIEF_MAX_BYTES)),
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();

        let task = build_task(&prof, &wt, "improve", "small ticket", None);

        assert!(task.contains(&format!(
            "Project brief truncated at {PROJECT_BRIEF_MAX_BYTES} bytes"
        )));
        assert!(!task.contains('é'));
    }

    #[test]
    fn labeled_freeform_issue_keeps_bounded_issue_description() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "297".to_string(),
            title: "Freeform issue".to_string(),
            body: "The non-structured reproduction and expected outcome live here.".to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

        let task = build_task(&prof, &wt, "fix", "#297", Some(&issue));

        assert!(task.contains("### Issue Description"));
        assert!(task.contains("non-structured reproduction"));
    }

    #[test]
    fn scope_and_invariants_headings_suppress_the_raw_body_fallback() {
        // Issue #405: an issue using `Scope`/`Invariants` instead of
        // `Problem`/`Constraints` is still structured -- the Live Task Pack
        // must carry the requirements, and the raw-body fallback (which
        // would otherwise duplicate them unbounded) must not also fire.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "378".to_string(),
            title: "Fix drift detection".to_string(),
            body: "## Scope\n\nDetect config drift across restarts\n\n\
                   ## Invariants\n\n- Never silently disable classification"
                .to_string(),
            labels: vec![],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#378", Some(&issue));

        assert!(task.contains("### Problem"));
        assert!(task.contains("Detect config drift across restarts"));
        assert!(task.contains("### Constraints"));
        assert!(task.contains("Never silently disable classification"));
        assert!(!task.contains("### Issue Description"));
    }

    #[test]
    fn markdown_goal_is_authoritative_over_scope_and_survives_the_task_pack() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "384".to_string(),
            title: "Preserve explicit goals".to_string(),
            body: "## Goal\n\nPreserve this goal.\n\n\
                   ## Scope\n\nFallback scope.\n\n\
                   ## Required Behavior\n\n- Keep behavior stable."
                .to_string(),
            labels: vec![],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#384", Some(&issue));

        assert!(task.contains("### Problem"));
        assert!(task.contains("Preserve this goal."));
        assert!(!task.contains("Fallback scope."));
        assert!(task.contains("Keep behavior stable."));
        assert!(!task.contains("### Issue Description"));
    }

    #[test]
    fn live_task_pack_caps_long_structured_sections_without_creating_headings() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "298".to_string(),
            title: "Large structured issue".to_string(),
            body: format!(
                "## Acceptance Criteria\n\n- {}\n\n## Constraints\n\n- keep scope\n",
                "x".repeat(LIVE_TASK_ACCEPTANCE_MAX_BYTES * 2)
            ),
            labels: vec![],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#298", Some(&issue));

        assert!(task.contains(&format!(
            "[List truncated at {LIVE_TASK_ACCEPTANCE_MAX_BYTES} bytes"
        )));
        assert!(task.contains("### Constraints"));
        assert_eq!(task.matches("## Focus").count(), 1);
    }

    #[test]
    fn candidate_task_places_no_push_guardrail_in_protected_safety_section() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let candidate = Candidate {
            candidate_id: "candidate-1".into(),
            source_gate_status: "ok".into(),
            suggested_blueprint_phase: "fix".into(),
            provider_mutation_allowed: false,
            suggested_labels: vec![],
            affected_files: vec!["## Injected files heading\npath.rs".into()],
            evidence: vec![
                "## Injected context heading\nmalicious content".into(),
                "x".repeat(10_000),
            ],
            acceptance_criteria: vec!["## Injected acceptance heading\nkeep safe".into()],
            verification: vec!["## Injected verification heading\nrun tests".into()],
            hydration_used: false,
            hydration_source: String::new(),
            hydration_match_method: String::new(),
            hydrated_fields: vec![],
            debug_gate_keys: vec![],
            debug_scout_keys: vec![],
            debug_hydrated_keys: vec![],
            debug_hydrated_finding_excerpt: String::new(),
            source_finding_path: None,
            source_draft_issue_path: None,
        };

        let task = format_candidate_task(&prof, tmp.path(), "improve", &candidate);
        for heading in [
            "Injected files heading",
            "Injected context heading",
            "Injected acceptance heading",
            "Injected verification heading",
        ] {
            assert!(task.contains(&format!("  ## {heading}")));
            assert!(!task.contains(&format!("\n## {heading}")));
        }
        let compacted = context::enforce(
            &task,
            &context::ContextConfig {
                soft_limit_tokens: 20,
                hard_limit_tokens: 200,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(compacted.prompt.contains("## Safety"));
        assert!(compacted.prompt.contains("Do not push or create MRs."));
    }

    #[test]
    fn improve_mode_with_a_target_says_ignore_other_backlog_items() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "TICKET-014: boost shots ROI", None);

        assert!(task.contains("Implement ONLY the specific ticket"));
        assert!(!task.contains("Select and implement the highest-priority"));
    }

    #[test]
    fn improve_mode_without_a_target_still_picks_from_backlog() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "", None);

        assert!(task.contains("Select and implement the highest-priority"));
    }
}
