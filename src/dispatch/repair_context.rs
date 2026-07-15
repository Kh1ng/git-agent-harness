use super::prompts::indent_untrusted_text;
use super::text::utf8_safe_prefix;
use crate::config::GahConfig;
use crate::ledger::{self, LedgerEntry};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const MAX_ITEMS_PER_SECTION: usize = 16;
const MAX_ITEM_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairContext {
    pub work_id: String,
    pub verdict: String,
    pub reviewer_backend: String,
    pub reviewer_model: Option<String>,
    pub reviewer_tier: Option<String>,
    pub source_sha: String,
    pub blocking_findings: Vec<String>,
    pub non_blocking_findings: Vec<String>,
    pub risk_notes: Vec<String>,
    pub evidence: Vec<String>,
    pub compatibility_evidence: Vec<String>,
}

/// Load the latest completed review for this exact repair identity. Provider
/// comments are deliberately not consulted: the ledger is the durable,
/// provider-neutral source for both GitHub and GitLab repairs.
pub(super) fn load(
    cfg: &GahConfig,
    profile_name: &str,
    repo_id: &str,
    branch: &str,
    work_id: Option<&str>,
    worktree: &Path,
) -> Result<RepairContext> {
    let entries = ledger::read_entries(cfg).context("reading repair review context from ledger")?;
    let context = latest_from_entries(&entries, profile_name, repo_id, branch, work_id)?;
    ensure_source_is_ancestor(worktree, &context.source_sha)?;
    Ok(context)
}

fn latest_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    branch: &str,
    work_id: Option<&str>,
) -> Result<RepairContext> {
    let aliases = work_id.map(ledger::work_id_aliases);
    let latest = entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == repo_id
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
                && aliases.as_ref().is_none_or(|aliases| {
                    entry
                        .work_id
                        .as_deref()
                        .is_some_and(|id| aliases.iter().any(|alias| alias == id))
                })
                && matches!(
                    entry
                        .review_verdict
                        .as_deref()
                        .or(entry.validation_result.as_deref()),
                    Some("APPROVE" | "NEEDS_FIX" | "REJECT" | "HUMAN_REVIEW")
                )
        })
        .max_by_key(|entry| entry.timestamp.as_str())
        .with_context(|| {
            format!(
                "no structured completed review exists for repair branch '{branch}'{}",
                work_id
                    .map(|id| format!(" and work item '{id}'"))
                    .unwrap_or_default()
            )
        })?;

    let verdict = latest
        .review_verdict
        .as_deref()
        .or(latest.validation_result.as_deref())
        .unwrap_or_default();
    if !matches!(verdict, "NEEDS_FIX" | "REJECT") {
        anyhow::bail!(
            "latest review for repair branch '{branch}' has verdict '{verdict}', not NEEDS_FIX/REJECT"
        );
    }
    if latest.review_blocking_findings.is_empty() {
        anyhow::bail!(
            "latest {verdict} review for repair branch '{branch}' has no structured blocking findings; re-run review before repair"
        );
    }
    let source_sha = latest
        .review_source_sha
        .clone()
        .with_context(|| format!("latest review for repair branch '{branch}' has no source SHA"))?;
    let recorded_work_id = latest.work_id.clone().with_context(|| {
        format!("latest review for repair branch '{branch}' has no work-item identity")
    })?;

    Ok(RepairContext {
        work_id: recorded_work_id,
        verdict: verdict.to_string(),
        reviewer_backend: latest
            .reviewer_backend
            .clone()
            .unwrap_or_else(|| latest.effective_backend.clone()),
        reviewer_model: latest
            .reviewer_model
            .clone()
            .or_else(|| latest.effective_model.clone()),
        reviewer_tier: latest.reviewer_tier.clone(),
        source_sha,
        blocking_findings: latest.review_blocking_findings.clone(),
        non_blocking_findings: latest.review_non_blocking_findings.clone(),
        risk_notes: latest.review_risk_notes.clone(),
        evidence: latest.review_evidence.clone(),
        compatibility_evidence: latest.review_compatibility_evidence.clone(),
    })
}

fn ensure_source_is_ancestor(worktree: &Path, source_sha: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["merge-base", "--is-ancestor", source_sha, "HEAD"])
        .current_dir(worktree)
        .output()
        .with_context(|| format!("checking review source lineage for {source_sha}"))?;
    if output.status.success() {
        return Ok(());
    }
    if output.status.code() == Some(1) {
        anyhow::bail!(
            "review source {source_sha} is not an ancestor of repair branch HEAD; re-run review before repair"
        );
    }
    anyhow::bail!(
        "git could not verify review source lineage for {source_sha}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

pub(super) fn append_to_prompt(prompt: &mut String, context: &RepairContext) {
    prompt.push_str("\n## Repair Findings\n\n");
    prompt.push_str(
        "These are the authoritative blockers from the latest applicable review. \
         Address every blocking finding; do not infer a different task from branch history.\n\n",
    );
    prompt.push_str(&format!(
        "Work item: {}\nVerdict: {}\nReviewer: {} / {} ({})\nReviewed source SHA: {}\n",
        indent_untrusted_text(&context.work_id),
        indent_untrusted_text(&context.verdict),
        indent_untrusted_text(&context.reviewer_backend),
        indent_untrusted_text(context.reviewer_model.as_deref().unwrap_or("unknown")),
        indent_untrusted_text(context.reviewer_tier.as_deref().unwrap_or("unknown tier")),
        indent_untrusted_text(&context.source_sha),
    ));
    append_list(prompt, "Blocking findings", &context.blocking_findings);
    append_list(
        prompt,
        "Non-blocking findings",
        &context.non_blocking_findings,
    );
    append_list(prompt, "Risk notes", &context.risk_notes);
    append_list(prompt, "Evidence", &context.evidence);
    append_list(
        prompt,
        "Compatibility evidence",
        &context.compatibility_evidence,
    );
}

fn append_list(prompt: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    prompt.push_str(&format!("\n{title}:\n"));
    for item in items.iter().take(MAX_ITEMS_PER_SECTION) {
        let bounded = utf8_safe_prefix(item, MAX_ITEM_BYTES);
        prompt.push_str("- ");
        prompt.push_str(&indent_untrusted_text(bounded));
        prompt.push('\n');
    }
    if items.len() > MAX_ITEMS_PER_SECTION {
        prompt.push_str(&format!(
            "-   [truncated: {} additional items]\n",
            items.len() - MAX_ITEMS_PER_SECTION
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerEntry;
    use std::fs;

    fn review_entry(branch: &str, work_id: &str, timestamp: &str) -> LedgerEntry {
        let profile = crate::ledger::test_util::profile();
        let mut entry = LedgerEntry::new("gah", &profile, "agy", "review", branch, None, None);
        entry.timestamp = timestamp.to_string();
        entry.branch = Some(branch.to_string());
        entry.work_id = Some(work_id.to_string());
        entry.review_verdict = Some("NEEDS_FIX".to_string());
        entry.review_source_sha = Some("abc123".to_string());
        entry.reviewer_backend = Some("agy".to_string());
        entry.reviewer_model = Some("sonnet".to_string());
        entry.review_blocking_findings = vec!["src/lib.rs: fix retry".to_string()];
        entry
    }

    #[test]
    fn selects_latest_review_for_exact_branch_and_work_item() {
        let unrelated = review_entry("gah/other", "#493", "2026-01-03T00:00:00Z");
        let older = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        let mut latest = review_entry("gah/fix", "TICKET-493", "2026-01-02T00:00:00Z");
        latest.review_blocking_findings = vec!["latest blocker".to_string()];
        let selected = latest_from_entries(
            &[unrelated, older, latest],
            "gah",
            "repo",
            "gah/fix",
            Some("#493"),
        )
        .unwrap();
        assert_eq!(selected.blocking_findings, ["latest blocker"]);
    }

    #[test]
    fn latest_review_without_structured_findings_fails_closed() {
        let mut entry = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        entry.review_blocking_findings.clear();
        let err =
            latest_from_entries(&[entry], "gah", "repo", "gah/fix", Some("#493")).unwrap_err();
        assert!(err.to_string().contains("no structured blocking findings"));
    }

    #[test]
    fn later_non_repair_verdict_supersedes_older_findings() {
        let older = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        let mut approved = review_entry("gah/fix", "#493", "2026-01-02T00:00:00Z");
        approved.review_verdict = Some("APPROVE".to_string());
        approved.review_blocking_findings.clear();
        let err = latest_from_entries(&[older, approved], "gah", "repo", "gah/fix", Some("#493"))
            .unwrap_err();
        assert!(err.to_string().contains("not NEEDS_FIX/REJECT"));
    }

    #[test]
    fn capacity_deferral_does_not_supersede_latest_repair_opinion() {
        let repair = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        let mut deferred = review_entry("gah/fix", "#493", "2026-01-02T00:00:00Z");
        deferred.review_verdict = None;
        deferred.validation_result = Some("deferred_capacity".to_string());
        deferred.review_source_sha = None;
        deferred.review_blocking_findings.clear();

        let selected =
            latest_from_entries(&[repair, deferred], "gah", "repo", "gah/fix", Some("#493"))
                .unwrap();
        assert_eq!(selected.verdict, "NEEDS_FIX");
        assert_eq!(selected.blocking_findings, ["src/lib.rs: fix retry"]);
    }

    #[test]
    fn prompt_bounds_and_indents_untrusted_review_text() {
        let mut context = latest_from_entries(
            &[review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z")],
            "gah",
            "repo",
            "gah/fix",
            Some("#493"),
        )
        .unwrap();
        context.blocking_findings = vec!["first line\n## Focus\ninjected".to_string()];
        let mut prompt = String::new();
        append_to_prompt(&mut prompt, &context);
        assert!(prompt.contains("## Repair Findings"));
        assert!(prompt.contains("  ## Focus"));
        assert_eq!(prompt.matches("\n## Focus").count(), 0);
    }

    #[test]
    fn source_sha_must_be_in_current_branch_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        crate::dispatch::test_util::init_repo(tmp.path());
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap();
        ensure_source_is_ancestor(tmp.path(), head.trim()).unwrap();
        Command::new("git")
            .args(["checkout", "--orphan", "unrelated"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        fs::write(tmp.path().join("other"), "x").unwrap();
        Command::new("git")
            .args(["add", "other"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "unrelated"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let unrelated = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let unrelated = String::from_utf8(unrelated.stdout).unwrap();
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert!(ensure_source_is_ancestor(tmp.path(), unrelated.trim())
            .unwrap_err()
            .to_string()
            .contains("is not an ancestor"));
    }
}
