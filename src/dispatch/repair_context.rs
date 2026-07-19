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
    pub metadata_fingerprint: String,
    pub review_contract_version: u32,
    pub review_generation: String,
    pub blocking_findings: Vec<String>,
    pub non_blocking_findings: Vec<String>,
    pub risk_notes: Vec<String>,
    pub evidence: Vec<String>,
    pub compatibility_evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RepairIdentity<'a> {
    pub profile_name: &'a str,
    pub repo_id: &'a str,
    pub branch: &'a str,
    pub work_id: Option<&'a str>,
    pub expected_review_generation: Option<&'a str>,
}

/// Load the latest applicable structured review for this exact repair
/// identity. Provider comments are deliberately not consulted: the ledger is
/// the durable, provider-neutral source for both GitHub and GitLab repairs.
/// Structured review verdicts may live on either a dedicated review entry or a
/// backfilled implementation entry, so both are considered.
pub(super) fn load(
    cfg: &GahConfig,
    profile: &crate::config::Profile,
    identity: RepairIdentity<'_>,
    worktree: &Path,
) -> Result<RepairContext> {
    if let Some(expected) = identity.expected_review_generation {
        let live = current_provider_generation(profile, identity.branch)?;
        if live.as_deref() != Some(expected) {
            anyhow::bail!(
                "repair branch '{}' changed after controller observation: expected review generation '{expected}', live provider generation is '{}'; re-run review before repair",
                identity.branch,
                live.as_deref().unwrap_or("unknown")
            );
        }
    }
    let entries = ledger::read_entries(cfg).context("reading repair review context from ledger")?;
    let context = latest_from_entries_for_generation(&entries, identity)?;
    ensure_source_is_ancestor(worktree, &context.source_sha)?;
    Ok(context)
}

fn current_provider_generation(
    profile: &crate::config::Profile,
    branch: &str,
) -> Result<Option<String>> {
    if !matches!(profile.provider.as_str(), "github" | "gitlab") {
        return Ok(None);
    }
    let target = crate::provider::find_review_target_by_branch(profile, branch)
        .with_context(|| format!("refreshing live review identity for repair branch '{branch}'"))?;
    let metadata_fingerprint = crate::sync::review_metadata_fingerprint(
        target.source_sha.as_deref(),
        target.title.as_deref(),
        target.body.as_deref(),
        target.draft,
    );
    Ok(crate::ledger::review_generation(
        target.source_sha.as_deref(),
        Some(&metadata_fingerprint),
    ))
}

#[cfg(test)]
fn latest_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    branch: &str,
    work_id: Option<&str>,
) -> Result<RepairContext> {
    latest_from_entries_for_generation(
        entries,
        RepairIdentity {
            profile_name,
            repo_id,
            branch,
            work_id,
            expected_review_generation: None,
        },
    )
}

fn latest_from_entries_for_generation(
    entries: &[LedgerEntry],
    identity: RepairIdentity<'_>,
) -> Result<RepairContext> {
    let aliases = identity.work_id.map(ledger::work_id_aliases);
    let latest = entries
        .iter()
        .filter(|entry| {
            entry.profile == identity.profile_name
                && entry.repo_id == identity.repo_id
                && matches!(entry.mode.as_str(), "review" | "fix" | "improve")
                && entry.review_contract_version == Some(ledger::REVIEW_CONTRACT_VERSION)
                && entry.review_generation.is_some()
                && identity
                    .expected_review_generation
                    .is_none_or(|expected| entry.review_generation.as_deref() == Some(expected))
                && entry.branch.as_deref() == Some(identity.branch)
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
                "no structured completed review exists for repair branch '{}'{}{}",
                identity.branch,
                identity
                    .work_id
                    .map(|id| format!(" and work item '{id}'"))
                    .unwrap_or_default(),
                identity
                    .expected_review_generation
                    .map(|generation| format!(" at expected generation '{generation}'"))
                    .unwrap_or_default(),
            )
        })?;

    let verdict = latest
        .review_verdict
        .as_deref()
        .or(latest.validation_result.as_deref())
        .unwrap_or_default();
    if !matches!(verdict, "NEEDS_FIX" | "REJECT") {
        anyhow::bail!(
            "latest review for repair branch '{}' has verdict '{verdict}', not NEEDS_FIX/REJECT",
            identity.branch
        );
    }
    if latest.review_blocking_findings.is_empty() {
        anyhow::bail!(
            "latest {verdict} review for repair branch '{}' has no structured blocking findings; re-run review before repair",
            identity.branch
        );
    }
    let source_sha = latest.review_source_sha.clone().with_context(|| {
        format!(
            "latest review for repair branch '{}' has no source SHA",
            identity.branch
        )
    })?;
    let metadata_fingerprint = latest
        .review_metadata_fingerprint
        .clone()
        .with_context(|| {
            format!(
                "latest review for repair branch '{}' has no metadata fingerprint",
                identity.branch
            )
        })?;
    let recorded_work_id = latest.work_id.clone().with_context(|| {
        format!(
            "latest review for repair branch '{}' has no work-item identity",
            identity.branch
        )
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
        metadata_fingerprint,
        review_contract_version: latest.review_contract_version.unwrap_or_default(),
        review_generation: latest.review_generation.clone().unwrap_or_default(),
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
        "Work item: {}\nVerdict: {}\nReviewer: {} / {} ({})\nReviewed source SHA: {}\nReview generation: {}\n",
        indent_untrusted_text(&context.work_id),
        indent_untrusted_text(&context.verdict),
        indent_untrusted_text(&context.reviewer_backend),
        indent_untrusted_text(context.reviewer_model.as_deref().unwrap_or("unknown")),
        indent_untrusted_text(context.reviewer_tier.as_deref().unwrap_or("unknown tier")),
        indent_untrusted_text(&context.source_sha),
        indent_untrusted_text(&context.review_generation),
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
    use crate::ledger::test_util as ledger_tests;
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
        entry.review_metadata_fingerprint = Some("sha256:test".to_string());
        entry.review_contract_version = Some(crate::ledger::REVIEW_CONTRACT_VERSION);
        entry.review_generation = crate::ledger::review_generation(
            entry.review_source_sha.as_deref(),
            entry.review_metadata_fingerprint.as_deref(),
        );
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
    fn controller_expected_generation_cannot_reuse_newer_or_older_review_findings() {
        let expected = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        let expected_generation = expected.review_generation.clone().unwrap();
        let mut different = review_entry("gah/fix", "#493", "2026-01-02T00:00:00Z");
        different.review_source_sha = Some("different-sha".into());
        different.review_metadata_fingerprint = Some("sha256:different".into());
        different.review_generation = crate::ledger::review_generation(
            different.review_source_sha.as_deref(),
            different.review_metadata_fingerprint.as_deref(),
        );
        different.review_blocking_findings = vec!["stale blocker".into()];

        let selected = latest_from_entries_for_generation(
            &[expected, different],
            RepairIdentity {
                profile_name: "gah",
                repo_id: "repo",
                branch: "gah/fix",
                work_id: Some("#493"),
                expected_review_generation: Some(&expected_generation),
            },
        )
        .unwrap();

        assert_eq!(selected.review_generation, expected_generation);
        assert_eq!(selected.blocking_findings, ["src/lib.rs: fix retry"]);
    }

    #[test]
    fn missing_controller_generation_fails_closed() {
        let entry = review_entry("gah/fix", "#493", "2026-01-01T00:00:00Z");
        let error = latest_from_entries_for_generation(
            &[entry],
            RepairIdentity {
                profile_name: "gah",
                repo_id: "repo",
                branch: "gah/fix",
                work_id: Some("#493"),
                expected_review_generation: Some("review-v1:missing:sha256:missing"),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("at expected generation"));
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
    fn existing_branch_preflight_uses_backfilled_implementation_review_context() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let repo = tempfile::tempdir().unwrap();
        let repo_path = repo.path().join("repo");
        crate::dispatch::test_util::init_repo(&repo_path);

        let source_sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        assert!(source_sha.status.success());
        let source_sha = String::from_utf8(source_sha.stdout).unwrap();
        let source_sha = source_sha.trim().to_string();

        fs::write(repo_path.join("README.md"), "updated\n").unwrap();
        let add = Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        assert!(add.status.success());
        let commit = Command::new("git")
            .args(["commit", "-m", "repair branch"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        assert!(commit.status.success());

        let profile = crate::dispatch::test_util::profile(&repo_path);
        let mut entry =
            LedgerEntry::new("test", &profile, "claude", "improve", "target", None, None);
        entry.branch = Some("gah/fix".to_string());
        entry.work_id = Some("#493".to_string());
        entry.review_verdict = Some("NEEDS_FIX".to_string());
        entry.review_confidence = Some("high".to_string());
        entry.reviewer_backend = Some("claude".to_string());
        entry.reviewer_model = Some("claude-sonnet-4".to_string());
        entry.reviewer_tier = Some("strong".to_string());
        entry.review_source_sha = Some(source_sha.clone());
        entry.review_metadata_fingerprint = Some("sha256:test".to_string());
        entry.review_contract_version = Some(crate::ledger::REVIEW_CONTRACT_VERSION);
        entry.review_generation = crate::ledger::review_generation(
            entry.review_source_sha.as_deref(),
            entry.review_metadata_fingerprint.as_deref(),
        );
        entry.review_blocking_findings = vec!["src/lib.rs: fix retry".to_string()];
        entry.review_non_blocking_findings = vec!["consider a smaller helper".to_string()];
        entry.review_risk_notes = vec!["retry state can be lost".to_string()];
        entry.review_evidence = vec!["file:src/lib.rs".to_string()];
        crate::ledger::append(&cfg, &entry).unwrap();

        let context = load(
            &cfg,
            &profile,
            RepairIdentity {
                profile_name: "test",
                repo_id: &profile.repo_id,
                branch: "gah/fix",
                work_id: Some("#493"),
                expected_review_generation: None,
            },
            &repo_path,
        )
        .unwrap();

        assert_eq!(context.work_id, "#493");
        assert_eq!(context.verdict, "NEEDS_FIX");
        assert_eq!(context.source_sha, source_sha);
        assert_eq!(context.reviewer_backend, "claude");
        assert_eq!(context.reviewer_model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(context.blocking_findings, ["src/lib.rs: fix retry"]);
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
