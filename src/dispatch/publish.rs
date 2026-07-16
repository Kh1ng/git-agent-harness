//! Publishing policy, source freshness checks, and deterministic handoff/MR rendering.

use super::already_satisfied::{
    classify_backend_disposition, reconcile_already_satisfied, AlreadySatisfiedEvidence,
    Disposition,
};
use super::issues::{fetch_issue_details, IssueDetails, TicketMetadata};
use crate::config::Profile;
use crate::ledger::LedgerEntry;
use crate::models::ReviewVerdict;
use std::path::Path;

pub(super) fn ensure_issue_open_for_publish(
    profile: &Profile,
    issue: &IssueDetails,
) -> anyhow::Result<()> {
    let fresh = fetch_issue_details(profile, &issue.number, false)?;
    match fresh.state.as_deref() {
        Some(state) if issue_state_is_open(state) => Ok(()),
        Some(state) => anyhow::bail!(
            "source issue #{} is {state}; refusing to publish completed or closed work",
            fresh.number
        ),
        None => anyhow::bail!(
            "source issue #{} did not report its state; refusing to publish without authoritative status",
            fresh.number
        ),
    }
}

fn issue_state_is_open(state: &str) -> bool {
    state.eq_ignore_ascii_case("open") || state.eq_ignore_ascii_case("opened")
}

pub(super) fn emit_human_handoff(
    profile: &Profile,
    ledger: &LedgerEntry,
    branch: &str,
    reason: &str,
) {
    println!("=== GAH human handoff (publishing policy) ===");
    println!("reason: {}", reason);
    println!("profile: {}", profile.display_name);
    println!("branch: {}", branch);
    println!(
        "validation_status: {}",
        ledger.validation_result.as_deref().unwrap_or("unknown")
    );
    println!("changed_files: {}", ledger.files_changed.unwrap_or(0));
    if let Some(verdict) = &ledger.review_verdict {
        println!("review_verdict: {}", verdict);
    }
    println!("=== end GAH human handoff ===");
}

/// TICKET-128: whether the profile may publish the work autonomously. A
/// restricted profile can still run the full backend and validation pipeline
/// and write local artifacts; they just must not be published as an agent-authored MR.
pub(super) fn publishing_allows_publish(profile: &Profile) -> bool {
    profile.publishing.allow_pull_request_creation
        && profile.publishing.allow_commit_message_generation
}

/// Stage the complete candidate tree and fail closed when a newly tracked path
/// matches the profile's generated-artifact policy. Staging is deliberate:
/// it makes unstaged additions, force-added ignored files, backend commits,
/// and rename destinations observable through one index-vs-target diff.
pub(super) fn enforce_generated_artifact_policy(
    profile: &Profile,
    ledger: &mut LedgerEntry,
    worktree_path: &Path,
) -> anyhow::Result<()> {
    if crate::worktree::has_uncommitted_changes(worktree_path)? {
        crate::worktree::stage_all(worktree_path)?;
    }
    if let Err(error) = crate::generated_artifacts::enforce_index_policy(
        worktree_path,
        &profile.default_target_branch,
        &profile.publishing.generated_artifact_deny_patterns,
    ) {
        ledger.set_failure(
            crate::ledger::FailureClass::HarnessError,
            crate::ledger::FailureStage::Push,
        );
        return Err(error);
    }
    Ok(())
}

/// Issue #584: the decision taken before a completion MR is published, when the
/// backend's disposition indicates the source work may already be satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AlreadySatisfiedPublishOutcome {
    /// Proceed with the normal completion-MR publication (the disposition was
    /// genuine agent-no-progress or a real, acceptable change).
    Proceed,
    /// The work is already satisfied and this profile is a trusted autonomous
    /// provider issue: post the grounded evidence and close the issue
    /// idempotently instead of publishing a completion MR.
    CloseIdempotently(AlreadySatisfiedEvidence),
    /// The work is already satisfied (or only a test-only regression diff exists)
    /// but GAH must not autonomously close. Emit a bounded operator handoff
    /// rather than a regressive completion MR.
    BoundedHandoff(String),
}

/// Issue #584: classify the backend's completion disposition and decide whether
/// a completion MR should be published, or whether an already-satisfied
/// disposition should be reconciled (idempotent close for trusted autonomous
/// provider issues, otherwise a bounded operator handoff). A test-only diff
/// that removes/weakens coverage is never accepted as completion of an
/// already-implemented production task.
pub(super) fn reconcile_before_publish(
    profile: &Profile,
    backend_summary: &str,
    diff: &super::already_satisfied::DiffSummary,
) -> AlreadySatisfiedPublishOutcome {
    match classify_backend_disposition(backend_summary, diff) {
        Disposition::AlreadySatisfied(evidence) => {
            match reconcile_already_satisfied(profile, &evidence) {
                super::already_satisfied::ReconciliationDecision::PostEvidenceAndClose {
                    ..
                } => AlreadySatisfiedPublishOutcome::CloseIdempotently(evidence),
                super::already_satisfied::ReconciliationDecision::BoundedOperatorHandoff {
                    reason,
                } => AlreadySatisfiedPublishOutcome::BoundedHandoff(reason),
            }
        }
        Disposition::RegressiveCompletion(_) => AlreadySatisfiedPublishOutcome::BoundedHandoff(
            "backend produced only a test-only diff that removes/weakens coverage; \
                 refusing to publish it as completion of an already-implemented production task"
                .into(),
        ),
        Disposition::AgentNoProgress => AlreadySatisfiedPublishOutcome::Proceed,
    }
}

pub(super) fn render_ticket_label(ticket: Option<&TicketMetadata>) -> String {
    let Some(ticket) = ticket else {
        return "n/a".into();
    };
    match (ticket.ticket_id.as_deref(), ticket.title.as_deref()) {
        (Some(ticket_id), Some(title)) => format!("{ticket_id} {title}"),
        (Some(ticket_id), None) => ticket_id.to_string(),
        (None, Some(title)) => title.to_string(),
        (None, None) => "n/a".into(),
    }
}

pub(super) fn format_validation_outcome(result: Option<&str>) -> &'static str {
    match result {
        Some("passed") => "passed",
        Some("failed-draft") => "failed, pushed as draft",
        Some("failed") => "failed",
        Some("not_run") => "not run",
        Some("answered") => "answered",
        Some("partial") => "partial",
        Some(_) => "recorded",
        None => "not recorded",
    }
}

pub(super) fn format_failure_state(ledger: &LedgerEntry) -> Option<String> {
    if let Some(summary) = ledger.error_summary.as_deref() {
        let mut state = String::new();
        if let Some(class) = ledger.failure_class.as_deref() {
            state.push_str(&format!("Class: `{class}`\n"));
        }
        if let Some(stage) = ledger.failure_stage.as_deref() {
            state.push_str(&format!("Stage: `{stage}`\n"));
        }
        state.push_str(&format!("Summary: {}", summary.trim()));
        return Some(state);
    }
    if ledger.validation_result.as_deref() == Some("failed-draft") {
        return Some("Validation remained red and the change was pushed as a draft.".into());
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_standard_mr_body(
    mode: &str,
    ticket: Option<&TicketMetadata>,
    backend: &str,
    model: &str,
    _branch: &str,
    _target_branch: &str,
    validation_passed: bool,
    backend_summary: &str,
) -> String {
    let mut sections = vec![format!(
        "## GAH {} mode\n\nTicket: {}\nBackend/model: `{}` / `{}`",
        mode,
        render_ticket_label(ticket),
        backend,
        model,
    )];

    // Add Closes directive for GitHub/GitLab auto-close if this came from an issue
    if let Some(ticket) = ticket {
        if let Some(ref issue_number) = ticket.issue_number {
            sections.push(format!("Closes #{}", issue_number));
        }
    }

    if !backend_summary.is_empty() {
        sections.push(format!("## What changed and why\n\n{}", backend_summary));
    }
    sections.push(format!(
        "Validation passed: {}\n\nGenerated by `gah dispatch`.",
        validation_passed,
    ));
    sections.join("\n\n")
}

pub(super) struct MrRenderContext<'a> {
    pub backend: &'a str,
    pub model: &'a str,
    pub branch: &'a str,
    pub target_branch: &'a str,
    pub validation_commands: &'a [String],
    pub ledger: &'a LedgerEntry,
    pub backend_summary: &'a str,
}

pub(super) fn build_metadata_rich_mr_body(
    mode: &str,
    ticket: &TicketMetadata,
    ctx: &MrRenderContext<'_>,
) -> String {
    let mut sections = Vec::new();

    let work_item = match (
        ticket.work_id.as_deref(),
        ticket.ticket_id.as_deref(),
        ticket.title.as_deref(),
    ) {
        (Some(id), _, Some(title)) => Some(format!("ID: `{id}`\nTitle: {title}")),
        (Some(id), _, None) => Some(format!("ID: `{id}`")),
        (None, Some(id), Some(title)) => Some(format!("ID: `{id}`\nTitle: {title}")),
        (None, Some(id), None) => Some(format!("ID: `{id}`")),
        (None, None, Some(title)) => Some(format!("Title: {title}")),
        (None, None, None) => None,
    };
    if let Some(section) = work_item {
        sections.push(format!("## Work Item\n\n{section}"));
    }

    // Add Closes directive for GitHub/GitLab auto-close if this came from an issue
    if let Some(ref issue_number) = ticket.issue_number {
        sections.push(format!("Closes #{}", issue_number));
    }

    if let Some(problem) = ticket.problem.as_deref() {
        sections.push(format!("## Problem\n\n{}", problem.trim()));
    }
    if let Some(goal) = ticket.goal.as_deref() {
        sections.push(format!("## Goal\n\n{}", goal.trim()));
    }
    if !ticket.acceptance_criteria.is_empty() {
        let lines = ticket
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Acceptance Criteria\n\n{lines}"));
    }
    if !ticket.constraints.is_empty() {
        let lines = ticket
            .constraints
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Constraints\n\n{lines}"));
    }
    if !ctx.backend_summary.is_empty() {
        sections.push(format!(
            "## What changed and why\n\n{}",
            ctx.backend_summary
        ));
    }
    if !ctx.validation_commands.is_empty() || ctx.ledger.validation_result.is_some() {
        let mut lines = vec![format!(
            "Outcome: {}",
            format_validation_outcome(ctx.ledger.validation_result.as_deref())
        )];
        for cmd in ctx.validation_commands {
            lines.push(format!("- `{cmd}`"));
        }
        sections.push(format!("## Validation\n\n{}", lines.join("\n")));
    }

    sections.push(format!(
        "## Backend / Model\n\nBackend: `{}`\nModel: `{}`",
        ctx.backend, ctx.model
    ));
    sections.push(format!(
        "## Attempts\n\nStarted: {}\nCompleted: {}\nFallback used: {}",
        ctx.ledger.attempts_started.unwrap_or(0),
        ctx.ledger.attempts_completed.unwrap_or(0),
        if ctx.ledger.fallback_used {
            "yes"
        } else {
            "no"
        }
    ));

    if let Some(state) = format_failure_state(ctx.ledger) {
        sections.push(format!("## Failure / Stop State\n\n{state}"));
    }

    if let Some(source) = ticket.source.as_deref() {
        sections.push(format!("## Source\n\n{}", source.trim()));
    }

    sections.push(format!("Generated by `gah dispatch --mode {mode}`."));
    sections.join("\n\n")
}

pub(super) fn build_fix_or_improve_mr_body(
    mode: &str,
    ticket: Option<&TicketMetadata>,
    ctx: &MrRenderContext<'_>,
    validation_passed: bool,
) -> String {
    match ticket {
        Some(ticket) => build_metadata_rich_mr_body(mode, ticket, ctx),
        None => build_standard_mr_body(
            mode,
            None,
            ctx.backend,
            ctx.model,
            ctx.branch,
            ctx.target_branch,
            validation_passed,
            ctx.backend_summary,
        ),
    }
}

pub(super) struct ExperimentMrRenderContext<'a> {
    pub backend: &'a str,
    pub model: &'a str,
    pub artifact_count: usize,
    pub answered: bool,
    pub backend_summary: &'a str,
}

pub(super) fn build_experiment_mr_body(ctx: &ExperimentMrRenderContext<'_>) -> String {
    let mut sections = vec![
        "## Experiment Result".to_string(),
        format!(
            "\nBackend: `{backend}`\nModel: `{model}`\nJudge verdict: {}\nArtifacts: {}\n",
            if ctx.answered { "answered" } else { "partial" },
            ctx.artifact_count,
            backend = ctx.backend,
            model = ctx.model
        ),
    ];
    if !ctx.backend_summary.is_empty() {
        sections.push(format!(
            "## What changed and why\n\n{}",
            ctx.backend_summary
        ));
    }
    sections.push("Generated by `gah dispatch --mode experiment`.".into());
    sections.join("\n\n")
}

pub(super) fn truncate_title(title: &str, limit: usize) -> String {
    if title.chars().count() <= limit {
        title.to_string()
    } else if limit <= 3 {
        ".".repeat(limit)
    } else {
        let mut truncated: String = title.chars().take(limit - 3).collect();
        truncated.push_str("...");
        truncated
    }
}

pub(super) fn build_mr_title(
    mode: &str,
    repo_id: &str,
    validation_failed: bool,
    ticket: Option<&TicketMetadata>,
) -> String {
    let mode_label = match mode {
        "fix" => "Fix",
        "improve" => "Improve",
        "review" => "Review",
        "pm" => "Plan",
        other => other,
    };
    let prefix = if validation_failed {
        "[GAH][DRAFT-FAIL]"
    } else {
        "[GAH]"
    };
    let title_string = if let Some(ticket) = ticket {
        let title_text = ticket
            .suggested_mr_title
            .as_deref()
            .or(ticket.title.as_deref());
        if let Some(title) = title_text {
            let detail = if ticket.is_authoritative {
                if let Some(ref work_id) = ticket.work_id {
                    format!("{work_id} {title}")
                } else if let Some(ref ticket_id) = ticket.ticket_id {
                    format!("{ticket_id} {title}")
                } else {
                    title.to_string()
                }
            } else {
                title.to_string()
            };
            format!("{prefix} {mode_label}: {detail}")
        } else {
            format!("{prefix} {mode_label}: {repo_id}")
        }
    } else {
        format!("{prefix} {mode_label}: {repo_id}")
    };
    truncate_title(&title_string, 255)
}

pub(super) fn render_review_comment(verdict: &ReviewVerdict, session_dir: &Path) -> String {
    let published = &verdict.verdict;
    let mut out = format!(
        "GAH review verdict: `{}`\n\nConfidence: `{}`\nHuman required: `{}`\nReviewer: `{}` / `{}`\nArtifacts: `{}`\n",
        published,
        verdict.confidence,
        verdict.human_required,
        verdict.effective_backend.as_deref().unwrap_or("unknown"),
        verdict.effective_model.as_deref().unwrap_or("unknown"),
        session_dir.display(),
    );
    if !verdict.blocking_findings.is_empty() {
        out.push_str("\nBlocking findings:\n");
        for item in &verdict.blocking_findings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    // A verdict with zero blocking findings (e.g. a low-confidence APPROVE)
    // still carries real substance in these two fields -- dropping them left
    // the posted PR comment as a bare verdict line with no actual feedback.
    if !verdict.non_blocking_findings.is_empty() {
        out.push_str("\nNon-blocking findings:\n");
        for item in &verdict.non_blocking_findings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !verdict.risk_notes.is_empty() {
        out.push_str("\nRisk notes:\n");
        for item in &verdict.risk_notes {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !verdict.evidence.is_empty() {
        out.push_str("\nEvidence:\n");
        for item in &verdict.evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !verdict.compatibility_evidence.is_empty() {
        out.push_str("\nCompatibility evidence:\n");
        for item in &verdict.compatibility_evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if let Some(reason) = &verdict.safety_gate_reason {
        out.push_str(&format!("\nGAH safety gate: {reason}\n"));
    }
    out
}

pub(super) fn review_labels(verdict: &ReviewVerdict) -> Vec<&'static str> {
    // TICKET-108: an APPROVE from a weak-tier reviewer, or a low-confidence
    // APPROVE from any reviewer, still needs human eyes -- reviewer identity
    // and the model's self-reported confidence are combined here, not
    // conflated into a single rewritten verdict string.
    let is_weak_tier = verdict.reviewer_tier.as_deref() == Some("weak");
    let is_low_confidence = verdict.confidence == "low";
    // A `HUMAN_REVIEW` text verdict can be a safety-gated APPROVE, an
    // uncertain reviewer, or an actual human handoff. `human_required` is
    // the controller decision after the bounded escalation chain has been
    // considered, so it is the authoritative distinction here.
    if verdict.human_required {
        if is_weak_tier || is_low_confidence {
            return vec!["gah-review-weak", "gah-human-review"];
        }
        return vec!["gah-human-review"];
    }
    match verdict.verdict.as_str() {
        "APPROVE" if is_weak_tier || is_low_confidence => vec!["gah-review-escalating"],
        "APPROVE" => vec!["gah-ready-for-human"],
        "NEEDS_FIX" | "REJECT" => vec!["gah-needs-fix"],
        "HUMAN_REVIEW" | "REVIEW_OUTPUT_INVALID" => vec!["gah-review-escalating"],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::test_util::profile;
    use crate::ledger::LedgerEntry;

    #[test]
    fn provider_open_issue_states_are_publishable() {
        for state in ["open", "OPEN", "opened", "OPENED"] {
            assert!(issue_state_is_open(state), "expected open state: {state}");
        }
        for state in ["closed", "completed", "locked", ""] {
            assert!(
                !issue_state_is_open(state),
                "expected terminal state: {state}"
            );
        }
    }

    #[test]
    fn invalid_review_output_keeps_provider_state_in_review_escalation() {
        let verdict: ReviewVerdict = serde_json::from_value(serde_json::json!({
            "verdict": "REVIEW_OUTPUT_INVALID",
            "confidence": "unknown",
            "human_required": false
        }))
        .unwrap();

        assert_eq!(review_labels(&verdict), ["gah-review-escalating"]);
    }

    #[test]
    fn generated_artifact_guard_marks_a_typed_push_failure_before_commit() {
        let temp = tempfile::tempdir().unwrap();
        crate::worktree::git(&["init", "-q"], temp.path()).unwrap();
        crate::worktree::git(
            &["config", "user.email", "gah@example.invalid"],
            temp.path(),
        )
        .unwrap();
        crate::worktree::git(&["config", "user.name", "GAH Test"], temp.path()).unwrap();
        std::fs::write(temp.path().join("README.md"), "base\n").unwrap();
        crate::worktree::git(&["add", "."], temp.path()).unwrap();
        crate::worktree::git(&["commit", "-q", "-m", "base"], temp.path()).unwrap();
        crate::worktree::git(&["branch", "-M", "main"], temp.path()).unwrap();
        crate::worktree::git(&["remote", "add", "origin", "."], temp.path()).unwrap();
        crate::worktree::git(
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            temp.path(),
        )
        .unwrap();
        let generated = temp
            .path()
            .join("apps/server/node_modules/.vite/vitest/results.json");
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        std::fs::write(generated, "{}\n").unwrap();

        let profile = profile(temp.path());
        let mut ledger = LedgerEntry::new(
            "real",
            &profile,
            "codex",
            "fix",
            "target",
            Some("session-artifact-guard".into()),
            None,
        );
        let error = enforce_generated_artifact_policy(&profile, &mut ledger, temp.path())
            .expect_err("generated dependency cache must be rejected");

        assert!(error
            .to_string()
            .contains("apps/server/node_modules/.vite/vitest/results.json"));
        assert!(error
            .to_string()
            .contains(crate::generated_artifacts::POLICY_SOURCE));
        assert_eq!(ledger.failure_class.as_deref(), Some("harness_error"));
        assert_eq!(ledger.failure_stage.as_deref(), Some("push"));
        assert!(crate::worktree::has_uncommitted_changes(temp.path()).unwrap());
    }

    #[test]
    fn title_preserves_authoritative_identity_and_draft_prefix() {
        let ticket = TicketMetadata {
            ticket_id: Some("#319".into()),
            work_id: Some("#319".into()),
            title: Some("Use native issue numbers".into()),
            issue_number: Some("319".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        assert_eq!(
            build_mr_title("fix", "repo", false, Some(&ticket)),
            "[GAH] Fix: #319 Use native issue numbers"
        );
        assert_eq!(
            build_mr_title("fix", "repo", true, Some(&ticket)),
            "[GAH][DRAFT-FAIL] Fix: #319 Use native issue numbers"
        );
    }

    #[test]
    fn standard_body_keeps_close_directive_and_summary_bytes() {
        let ticket = TicketMetadata {
            issue_number: Some("319".into()),
            title: Some("Native issue".into()),
            ..TicketMetadata::default()
        };
        let body = build_standard_mr_body(
            "fix",
            Some(&ticket),
            "codex",
            "gpt",
            "branch",
            "main",
            true,
            "Changed one thing.",
        );

        assert!(body.contains("Closes #319"));
        assert!(body.contains("## What changed and why\n\nChanged one thing."));
        assert!(body.ends_with("Validation passed: true\n\nGenerated by `gah dispatch`."));
    }

    #[test]
    fn review_comment_keeps_non_blocking_findings_risks_and_gate_reason_once() {
        let mut verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"HUMAN_REVIEW","confidence":"low","human_required":true,
                "blocking_findings":[],
                "non_blocking_findings":["missing test coverage"],
                "risk_notes":["new module coupling"]}"#,
        )
        .unwrap();
        verdict.safety_gate_reason = Some("APPROVE omitted grounded evidence".into());

        let comment = render_review_comment(&verdict, Path::new("/tmp/session"));
        assert!(comment.contains("missing test coverage"));
        assert!(comment.contains("new module coupling"));
        assert_eq!(
            comment.matches("APPROVE omitted grounded evidence").count(),
            1
        );
    }

    #[test]
    fn mr_title_uses_ticket_context_and_preserves_draft_fail_prefix() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-058".into()),
            work_id: Some("TICKET-058".into()),
            title: Some("Descriptive MR Titles".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        assert_eq!(
            build_mr_title("fix", "real", false, Some(&ticket)),
            "[GAH] Fix: TICKET-058 Descriptive MR Titles"
        );
        assert_eq!(
            build_mr_title("fix", "real", true, Some(&ticket)),
            "[GAH][DRAFT-FAIL] Fix: TICKET-058 Descriptive MR Titles"
        );
    }

    #[test]
    fn mr_title_uses_native_issue_identity_without_ticket_alias() {
        let ticket = TicketMetadata {
            ticket_id: Some("#319".into()),
            work_id: Some("#319".into()),
            title: Some("Use native issue numbers".into()),
            issue_number: Some("319".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        assert_eq!(
            build_mr_title("fix", "real", false, Some(&ticket)),
            "[GAH] Fix: #319 Use native issue numbers"
        );
    }

    #[test]
    fn render_review_comment_includes_non_blocking_findings_and_risk_notes() {
        // Regression: a verdict with zero blocking_findings (e.g. a
        // low-confidence APPROVE) still carries real substance in these two
        // fields. The posted PR comment was silently dropping both, leaving
        // reviewers with nothing but a bare verdict/confidence line and no
        // actual feedback.
        let verdict: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":"low","human_required":true,
                "blocking_findings":[],
                "non_blocking_findings":["missing test coverage on one path"],
                "risk_notes":["new module coupling"]}"#,
        )
        .unwrap();
        let comment = render_review_comment(&verdict, Path::new("/tmp/session"));
        assert!(comment.contains("Non-blocking findings:"));
        assert!(comment.contains("missing test coverage on one path"));
        assert!(comment.contains("Risk notes:"));
        assert!(comment.contains("new module coupling"));
    }

    #[test]
    fn render_review_comment_prints_gate_reason_once() {
        let mut verdict: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"HUMAN_REVIEW","confidence":"high","human_required":true,
                "blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        )
        .unwrap();
        verdict.safety_gate_reason = Some("APPROVE omitted grounded evidence".into());

        let comment = render_review_comment(&verdict, Path::new("/tmp/session"));
        assert_eq!(
            comment.matches("APPROVE omitted grounded evidence").count(),
            1
        );
    }

    #[test]
    fn mr_title_missing_metadata_fallback() {
        // Without ticket metadata, it should fall back to mode + repo_id
        let title = build_mr_title("fix", "real", false, None);
        assert_eq!(title, "[GAH] Fix: real");

        let title_draft = build_mr_title("fix", "real", true, None);
        assert_eq!(title_draft, "[GAH][DRAFT-FAIL] Fix: real");
    }

    #[test]
    fn mr_title_suggested_mr_title_used() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some("Heading Title".into()),
            suggested_mr_title: Some(
                "Derive PR titles from authoritative structured work metadata".into(),
            ),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        // When suggested_mr_title is present and authoritative, use it with the ID
        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert_eq!(
            title,
            "[GAH] Fix: TICKET-093 Derive PR titles from authoritative structured work metadata"
        );
    }

    #[test]
    fn mr_title_graceful_truncation() {
        let long_title = "a".repeat(300);
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some(long_title),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert!(title.chars().count() <= 255);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn mr_title_unicode_truncation_uses_character_limit() {
        let ticket = TicketMetadata {
            ticket_id: Some("#159".into()),
            work_id: Some("#159".into()),
            title: Some("é".repeat(300)),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert_eq!(title.chars().count(), 255);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn metadata_rich_mr_body_includes_structured_sections() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-094".into()),
            work_id: Some("TICKET-094".into()),
            title: Some("Authoritative PR Description".into()),
            summary: Some("Authoritative PR Description".into()),
            problem: Some("The old MR body only showed a minimal template.".into()),
            goal: Some("Generate PR descriptions from structured metadata.".into()),
            acceptance_criteria: vec![
                "Description includes structured sections".into(),
                "Legacy fallback remains available".into(),
            ],
            constraints: vec!["Do not dump raw prompts".into()],
            source: Some("docs/tickets/TICKET-094-authoritative-pr-description.md".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );
        ledger.validation_result = Some("passed".into());
        ledger.files_changed = Some(2);
        ledger.insertions = Some(14);
        ledger.deletions = Some(3);
        ledger.attempts_started = Some(2);
        ledger.attempts_completed = Some(2);
        ledger.fallback_used = true;

        let validation_commands = vec!["cargo test".into(), "cargo fmt --check".into()];
        let backend_summary = "Fixed the PR description to include reasoning.";
        let ctx = MrRenderContext {
            backend: "codex",
            model: "gpt-5.4",
            branch: "gah/repo-123",
            target_branch: "main",
            validation_commands: &validation_commands,
            ledger: &ledger,
            backend_summary,
        };
        let body = build_fix_or_improve_mr_body("fix", Some(&ticket), &ctx, true);

        assert!(body.contains("## Work Item"));
        assert!(body.contains("ID: `TICKET-094`"));
        assert!(body.contains("## Problem"));
        assert!(body.contains("The old MR body only showed a minimal template."));
        assert!(body.contains("## Goal"));
        assert!(body.contains("## Acceptance Criteria"));
        assert!(body.contains("- Description includes structured sections"));
        assert!(body.contains("## Constraints"));
        assert!(body.contains("- Do not dump raw prompts"));
        assert!(body.contains("## What changed and why"));
        assert!(body.contains("Fixed the PR description to include reasoning."));
        assert!(body.contains("## Validation"));
        assert!(body.contains("Outcome: passed"));
        assert!(body.contains("- `cargo test`"));
        assert!(body.contains("## Backend / Model"));
        assert!(body.contains("## Attempts"));
        assert!(body.contains("Fallback used: yes"));
        assert!(body.contains("## Source"));
        assert!(body.contains("docs/tickets/TICKET-094-authoritative-pr-description.md"));
        assert!(!body.contains("## Changes"));
        assert!(!body.contains("## Branch"));
        assert!(!body.contains("## Failure / Stop State"));
    }

    #[test]
    fn metadata_poor_mr_body_falls_back_to_legacy_template() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let validation_commands = Vec::new();
        let backend_summary = "Fixed the issue.";
        let ctx = MrRenderContext {
            backend: "codex",
            model: "gpt-5.4",
            branch: "gah/repo-123",
            target_branch: "main",
            validation_commands: &validation_commands,
            ledger: &ledger,
            backend_summary,
        };
        let body = build_fix_or_improve_mr_body("fix", None, &ctx, true);

        assert!(body.contains("## GAH fix mode"));
        assert!(body.contains("Ticket: n/a"));
        assert!(body.contains("Validation passed: true"));
        assert!(!body.contains("## Work Item"));
    }

    #[test]
    fn metadata_rich_mr_body_includes_closes_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: Some("72".to_string()),
            ..TicketMetadata::default()
        };

        let ctx = MrRenderContext {
            backend: "test",
            model: "test-model",
            branch: "gah/test-123",
            target_branch: "main",
            validation_commands: &[],
            ledger: &ledger,
            backend_summary: "Test summary",
        };

        let body = build_metadata_rich_mr_body("fix", &ticket, &ctx);

        // Verify that the Closes directive is included
        assert!(
            body.contains("Closes #72"),
            "MR body should contain 'Closes #72'"
        );

        // Verify it's not at the very beginning or end (should be after Work Item section)
        assert!(
            !body.starts_with("Closes #72"),
            "Closes directive should not be at the start"
        );
    }

    #[test]
    fn standard_mr_body_includes_closes_directive() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: Some("72".to_string()),
            ..TicketMetadata::default()
        };

        let body = build_standard_mr_body(
            "fix",
            Some(&ticket),
            "test",
            "test-model",
            "branch",
            "main",
            true,
            "Test summary",
        );

        // Verify that the Closes directive is included
        assert!(
            body.contains("Closes #72"),
            "Standard MR body should contain 'Closes #72'"
        );
    }

    #[test]
    fn mr_body_no_closes_directive_without_issue_number() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: None, // No issue number
            ..TicketMetadata::default()
        };

        let body = build_standard_mr_body(
            "fix",
            Some(&ticket),
            "test",
            "test-model",
            "branch",
            "main",
            true,
            "Test summary",
        );

        // Verify that the Closes directive is NOT included when there's no issue number
        assert!(
            !body.contains("Closes #"),
            "Standard MR body should not contain Closes directive without issue number"
        );
    }

    #[test]
    fn metadata_rich_mr_body_no_closes_directive_without_issue_number() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let ticket = TicketMetadata {
            ticket_id: None,
            work_id: None,
            title: Some("Test Issue".to_string()),
            issue_number: None, // No issue number
            ..TicketMetadata::default()
        };

        let ctx = MrRenderContext {
            backend: "test",
            model: "test-model",
            branch: "gah/test-123",
            target_branch: "main",
            validation_commands: &[],
            ledger: &ledger,
            backend_summary: "Test summary",
        };

        let body = build_metadata_rich_mr_body("fix", &ticket, &ctx);

        // Verify that the Closes directive is NOT included when there's no issue number
        assert!(
            !body.contains("Closes #"),
            "MR body should not contain Closes directive without issue number"
        );
    }
}
