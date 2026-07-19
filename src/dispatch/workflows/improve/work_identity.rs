use crate::dispatch::issues::TicketMetadata;
use crate::ledger::LedgerEntry;
use crate::{config, ledger};
use anyhow::Context;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ManualFixContext {
    pub existing_branch: Option<String>,
    pub work_id: Option<String>,
    pub source_issue_number: Option<String>,
    pub mr_url: Option<String>,
}

/// Apply an authoritative external identity, retain a controller-supplied
/// FixMr identity, or fall back to the unique dispatch branch.
pub(super) fn apply_authoritative_work_identity(
    ledger: &mut LedgerEntry,
    ticket: Option<&TicketMetadata>,
    fallback_work_id: &str,
) {
    if let Some(ticket) = ticket {
        ledger.task_class = ticket.task_class.clone();
        ledger.difficulty = ticket.difficulty.clone();
    }
    match ticket {
        Some(ticket) if ticket.is_authoritative => {
            ledger.work_id = ticket.work_id.clone().or_else(|| ticket.ticket_id.clone());
            ledger.source_issue_number = ticket.issue_number.clone();
            ledger.work_title = ticket.title.clone();
        }
        _ if ledger
            .work_id
            .as_deref()
            .is_some_and(|work_id| !work_id.trim().is_empty()) => {}
        _ => ledger.work_id = Some(fallback_work_id.to_string()),
    }
}

pub(super) fn resolve_manual_fix_work_identity(
    cfg: &config::GahConfig,
    profile_name: &str,
    branch: &str,
) -> Result<(String, Option<String>)> {
    let profile = config::get_profile(cfg, profile_name)?;
    let entries = ledger::read_entries(cfg)?;

    let mut relevant = entries
        .into_iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && entry.branch.as_deref() == Some(branch)
                && matches!(
                    entry.mode.as_str(),
                    "review" | "fix" | "improve" | "implement"
                )
                && entry
                    .work_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
        })
        .collect::<Vec<_>>();

    if relevant.is_empty() {
        anyhow::bail!(
            "could not resolve a work identity for branch '{branch}' from existing ledger records; \
no prior implementation/review dispatch record on this branch.",
        );
    }

    let mut by_work_id: HashMap<String, Vec<LedgerEntry>> = HashMap::new();
    for entry in relevant.drain(..) {
        let Some(work_id) = entry.work_id.as_deref() else {
            continue;
        };
        let canonical = canonical_work_id(work_id);
        by_work_id.entry(canonical).or_default().push(entry);
    }

    if by_work_id.is_empty() {
        anyhow::bail!(
            "could not resolve a work identity for branch '{branch}' from existing ledger records; \
no prior implementation/review dispatch record on this branch.",
        );
    }

    if by_work_id.len() > 1 {
        let mut ids: Vec<_> = by_work_id.keys().cloned().collect();
        ids.sort_unstable();
        anyhow::bail!(
            "MR source branch '{branch}' has multiple work identities in the ledger: {:?}. Pass an explicit work-target branch/revision to the controller or cleanup duplicate branch state before retrying manual repair.",
            ids
        );
    }

    let (canonical_work_id, mut records) = by_work_id
        .into_iter()
        .next()
        .context("resolved no work identity for manual FixMr")?;
    records.sort_by_key(|entry| entry.timestamp.clone());

    let source_issue_number = records
        .into_iter()
        .rev()
        .find_map(|entry| entry.source_issue_number)
        .and_then(|value| canonical_source_issue(&value))
        .or_else(|| canonical_source_issue(&canonical_work_id));

    Ok((canonical_work_id, source_issue_number))
}

pub(super) fn resolve_manual_fix_context(
    cfg: &config::GahConfig,
    profile_name: &str,
    profile: &config::Profile,
    mode: &str,
    existing_branch: Option<String>,
    mr: Option<&str>,
) -> Result<ManualFixContext> {
    let mut existing_branch = existing_branch;
    let mut manual_fix_work_id: Option<String> = None;
    let mut manual_fix_source_issue: Option<String> = None;
    let mut manual_fix_mr_url: Option<String> = None;

    if mode == "fix" {
        if let Some(mr) = mr {
            let review_target = crate::provider::find_review_target_by_mr(profile, mr)
                .with_context(|| format!("resolve source branch for MR {mr}"))?;
            if review_target.source_branch.trim().is_empty() {
                anyhow::bail!(
                    "MR {mr} is missing a source branch; cannot resolve FixMr branch target"
                );
            }
            ensure_manual_fix_review_target_is_open(mr, &review_target)
                .with_context(|| format!("validate source branch for MR {mr}"))?;
            if let Some(explicit_branch) = existing_branch.as_deref() {
                if explicit_branch != review_target.source_branch {
                    anyhow::bail!(
                        "--existing-branch '{explicit_branch}' does not match MR {mr} source branch '{}'; remove --existing-branch or pass the exact MR source branch",
                        review_target.source_branch
                    );
                }
            }
            println!(
                "Resolved MR {} to branch {}",
                review_target.id.as_str(),
                review_target.source_branch
            );
            manual_fix_mr_url = Some(review_target.url);
            existing_branch = Some(review_target.source_branch.clone());
            let (work_id, source_issue_number) =
                resolve_manual_fix_work_identity(cfg, profile_name, &review_target.source_branch)
                    .with_context(|| {
                    format!(
                        "resolve work identity for branch {}",
                        review_target.source_branch
                    )
                })?;
            manual_fix_work_id = Some(work_id);
            manual_fix_source_issue = source_issue_number;
        }
    }

    Ok(ManualFixContext {
        existing_branch,
        work_id: manual_fix_work_id,
        source_issue_number: manual_fix_source_issue,
        mr_url: manual_fix_mr_url,
    })
}

pub(super) fn resolve_target(
    args: &crate::dispatch::DispatchArgs,
    profile: &config::Profile,
    manual_fix: &ManualFixContext,
) -> Result<String> {
    if !args.target.is_empty() {
        return Ok(args.target.clone());
    }
    if args.mode == "fix" && args.mr.is_some() {
        return manual_fix
            .work_id
            .clone()
            .context("manual FixMr resolved no work identity to use as the repair target");
    }

    let default = PathBuf::from(&profile.artifact_root)
        .join("candidates")
        .join("latest.json");
    if default.exists() {
        println!("Auto-target: {}", default.display());
        Ok(default.to_string_lossy().into_owned())
    } else {
        Ok(args.target.clone())
    }
}

pub(super) fn apply_manual_fix_context_to_ledger(
    ledger: &mut LedgerEntry,
    ticket_meta: Option<&TicketMetadata>,
    branch: &str,
    manual_fix_context: &ManualFixContext,
) {
    let fallback_work_id = manual_fix_context
        .work_id
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| branch.to_string());
    apply_authoritative_work_identity(ledger, ticket_meta, &fallback_work_id);
    if let Some(work_id) = manual_fix_context.work_id.as_deref() {
        ledger.work_id = Some(work_id.to_string());
    }
    if let Some(source_issue_number) = manual_fix_context.source_issue_number.as_deref() {
        ledger.source_issue_number = Some(source_issue_number.to_string());
    }
    if let Some(mr_url) = manual_fix_context.mr_url.as_deref() {
        ledger.mr_url = Some(mr_url.to_string());
    }
}

fn canonical_work_id(work_id: &str) -> String {
    let trimmed = work_id.trim();

    if let Some(raw_number) = trimmed
        .strip_prefix('#')
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    {
        return format!("TICKET-{raw_number}");
    }

    if let Some(raw_number) = trimmed
        .strip_prefix("TICKET-")
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    {
        return format!("TICKET-{raw_number}");
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return format!("TICKET-{trimmed}");
    }

    trimmed.to_string()
}

fn canonical_source_issue(work_id_or_issue: &str) -> Option<String> {
    let trimmed = work_id_or_issue.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(raw_number) = trimmed
        .strip_prefix('#')
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    {
        return Some(raw_number.to_string());
    }

    if let Some(number) = trimmed
        .strip_prefix("TICKET-")
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    {
        return Some(number.to_string());
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    None
}

fn ensure_manual_fix_review_target_is_open(
    mr: &str,
    review_target: &crate::provider::ReviewTarget,
) -> Result<()> {
    if review_target
        .merged_at
        .as_deref()
        .is_some_and(|merged_at| !merged_at.trim().is_empty())
    {
        anyhow::bail!(
            "MR {mr} is merged and cannot be reused for fix repair; provide an open, unmerged manual-fix MR or pass --existing-branch explicitly"
        );
    }

    let state = review_target
        .state
        .as_deref()
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "MR {mr} provider response omitted its state; refusing to reuse an unverified repair branch"
            )
        })?;
    let state = state.to_ascii_lowercase();
    if !matches!(state.as_str(), "open" | "opened") {
        anyhow::bail!(
            "MR {mr} is in state {state} and cannot be reused for fix repair; provide an open, unmerged manual-fix MR or pass --existing-branch explicitly"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::test_util as ledger_test_util;

    fn build_entries(profile: &crate::config::Profile) -> Vec<crate::ledger::LedgerEntry> {
        let mut entries = vec![];
        let mut review = crate::ledger::LedgerEntry::new(
            "test",
            profile,
            "openhands",
            "review",
            "gah/manual-fix-1",
            None,
            None,
        );
        review.work_id = Some("#269".into());
        review.review_verdict = Some("NEEDS_FIX".into());
        review.review_source_sha = Some("HEAD".into());
        review.review_blocking_findings = vec!["repro test".into()];
        review.branch = Some("gah/manual-fix-1".into());
        review.timestamp = "2026-07-01T00:00:00Z".into();
        entries.push(review);

        let mut later = crate::ledger::LedgerEntry::new(
            "test",
            profile,
            "openhands",
            "review",
            "gah/manual-fix-1",
            None,
            None,
        );
        later.work_id = Some("TICKET-269".into());
        later.source_issue_number = Some("169".into());
        later.review_verdict = Some("NEEDS_FIX".into());
        later.review_source_sha = Some("HEAD".into());
        later.review_blocking_findings = vec!["repro test".into()];
        later.branch = Some("gah/manual-fix-1".into());
        later.timestamp = "2026-07-02T00:00:00Z".into();
        entries.push(later);

        entries
    }

    fn review_target_with_state(state: Option<&str>) -> crate::provider::ReviewTarget {
        crate::provider::ReviewTarget {
            id: "269".into(),
            url: "https://example.test/merge_requests/269".into(),
            source_branch: "gah/manual-fix-1".into(),
            target_branch: "main".into(),
            state: state.map(str::to_string),
            merged_at: None,
            title: None,
            body: None,
            draft: false,
            ci_status: None,
            merge_status: None,
            source_sha: None,
            target_sha: None,
        }
    }

    #[test]
    fn manual_fix_rejects_provider_target_without_state() {
        let error = ensure_manual_fix_review_target_is_open("269", &review_target_with_state(None))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("provider response omitted its state"));
    }

    #[test]
    fn manual_fix_identity_overrides_unrelated_candidate_metadata() {
        let profile = ledger_test_util::profile();
        let mut entry = crate::ledger::LedgerEntry::new(
            "test",
            &profile,
            "openhands",
            "fix",
            "gah/manual-fix-1",
            None,
            None,
        );
        let candidate = TicketMetadata {
            work_id: Some("TICKET-999".into()),
            ticket_id: Some("TICKET-999".into()),
            title: Some("unrelated stale candidate".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        let context = ManualFixContext {
            existing_branch: Some("gah/manual-fix-1".into()),
            work_id: Some("TICKET-269".into()),
            source_issue_number: Some("269".into()),
            mr_url: Some("https://example.test/merge_requests/269".into()),
        };

        apply_manual_fix_context_to_ledger(
            &mut entry,
            Some(&candidate),
            "gah/manual-fix-1",
            &context,
        );

        assert_eq!(entry.work_id.as_deref(), Some("TICKET-269"));
        assert_eq!(entry.source_issue_number.as_deref(), Some("269"));
    }

    #[test]
    fn canonicalizes_work_ids_and_source_issue_forms() {
        assert_eq!(canonical_work_id("#269"), "TICKET-269");
        assert_eq!(canonical_work_id("TICKET-269"), "TICKET-269");
        assert_eq!(canonical_work_id("269"), "TICKET-269");
        assert_eq!(canonical_work_id("feature/ticket"), "feature/ticket");

        assert_eq!(canonical_source_issue("#269"), Some("269".into()));
        assert_eq!(canonical_source_issue("TICKET-269"), Some("269".into()));
        assert_eq!(canonical_source_issue("269"), Some("269".into()));
        assert_eq!(canonical_source_issue("ticket-269"), None);
    }

    #[test]
    fn resolve_manual_fix_work_identity_prefers_canonical_work_id() {
        let (_tmp, mut cfg) = ledger_test_util::test_config();
        let mut profile = ledger_test_util::profile();
        profile.repo_id = "test".into();
        cfg.profiles.insert("test".into(), profile.clone());

        let path = cfg.defaults.ledger_path();
        for entry in build_entries(&profile) {
            let mut content = String::new();
            if path.exists() {
                content = std::fs::read_to_string(&path).unwrap();
            }
            content.push_str(&serde_json::to_string(&entry).unwrap());
            content.push('\n');
            std::fs::write(&path, content).unwrap();
        }

        let (work_id, source_issue) =
            resolve_manual_fix_work_identity(&cfg, "test", "gah/manual-fix-1").unwrap();

        assert_eq!(work_id, "TICKET-269");
        assert_eq!(source_issue, Some("169".into()));
    }

    #[test]
    fn resolve_manual_fix_work_identity_falls_back_to_canonical_source_issue() {
        let (_tmp, mut cfg) = ledger_test_util::test_config();
        let mut profile = ledger_test_util::profile();
        profile.repo_id = "test".into();
        cfg.profiles.insert("test".into(), profile.clone());

        let path = cfg.defaults.ledger_path();
        let mut entry = crate::ledger::LedgerEntry::new(
            "test",
            &profile,
            "openhands",
            "review",
            "gah/manual-fix-2",
            None,
            None,
        );
        entry.timestamp = "2026-07-01T00:00:00Z".into();
        entry.work_id = Some("#269".into());
        entry.review_verdict = Some("NEEDS_FIX".into());
        entry.review_source_sha = Some("HEAD".into());
        entry.review_blocking_findings = vec!["repro test".into()];
        entry.branch = Some("gah/manual-fix-2".into());
        let mut content = String::new();
        if path.exists() {
            content = std::fs::read_to_string(&path).unwrap();
        }
        content.push_str(&serde_json::to_string(&entry).unwrap());
        content.push('\n');
        std::fs::write(&path, content).unwrap();

        let (work_id, source_issue) =
            resolve_manual_fix_work_identity(&cfg, "test", "gah/manual-fix-2").unwrap();

        assert_eq!(work_id, "TICKET-269");
        assert_eq!(source_issue, Some("269".into()));
    }

    #[test]
    fn resolve_manual_fix_work_identity_considers_implement_mode() {
        let (_tmp, mut cfg) = ledger_test_util::test_config();
        let mut profile = ledger_test_util::profile();
        profile.repo_id = "test".into();
        cfg.profiles.insert("test".into(), profile.clone());

        let path = cfg.defaults.ledger_path();
        let mut entry = crate::ledger::LedgerEntry::new(
            "test",
            &profile,
            "openhands",
            "implement",
            "gah/manual-fix-3",
            None,
            None,
        );
        entry.work_id = Some("#269".into());
        entry.review_verdict = Some("NEEDS_FIX".into());
        entry.review_source_sha = Some("HEAD".into());
        entry.review_blocking_findings = vec!["repro test".into()];
        entry.branch = Some("gah/manual-fix-3".into());
        entry.timestamp = "2026-07-01T00:00:00Z".into();

        let mut content = String::new();
        if path.exists() {
            content = std::fs::read_to_string(&path).unwrap();
        }
        content.push_str(&serde_json::to_string(&entry).unwrap());
        content.push('\n');
        std::fs::write(&path, content).unwrap();

        let (work_id, source_issue) =
            resolve_manual_fix_work_identity(&cfg, "test", "gah/manual-fix-3").unwrap();

        assert_eq!(work_id, "TICKET-269");
        assert_eq!(source_issue, Some("269".into()));
    }

    #[test]
    fn resolve_manual_fix_work_identity_reports_ambiguous_work_identifiers() {
        let (_tmp, mut cfg) = ledger_test_util::test_config();
        let mut profile = ledger_test_util::profile();
        profile.repo_id = "test".into();
        cfg.profiles.insert("test".into(), profile.clone());

        let path = cfg.defaults.ledger_path();
        for work_id in ["#269", "TICKET-270"] {
            let mut entry = crate::ledger::LedgerEntry::new(
                "test",
                &profile,
                "openhands",
                "review",
                "gah/manual-fix-1",
                None,
                None,
            );
            entry.timestamp = "2026-07-01T00:00:00Z".into();
            entry.work_id = Some(work_id.to_string());
            entry.review_verdict = Some("NEEDS_FIX".into());
            entry.review_source_sha = Some("HEAD".into());
            entry.review_blocking_findings = vec!["repro test".into()];
            entry.branch = Some("gah/manual-fix-1".into());
            let mut content = String::new();
            if path.exists() {
                content = std::fs::read_to_string(&path).unwrap();
            }
            content.push_str(&serde_json::to_string(&entry).unwrap());
            content.push('\n');
            std::fs::write(&path, content).unwrap();
        }

        let err = resolve_manual_fix_work_identity(&cfg, "test", "gah/manual-fix-1").unwrap_err();

        assert!(err.to_string().contains("multiple work identities"));
    }

    #[test]
    fn resolve_manual_fix_work_identity_reports_missing_identity() {
        let (_tmp, mut cfg) = ledger_test_util::test_config();
        cfg.profiles
            .insert("test".into(), ledger_test_util::profile());

        let err = resolve_manual_fix_work_identity(&cfg, "test", "gah/missing").unwrap_err();

        assert!(err
            .to_string()
            .contains("could not resolve a work identity"));
    }
}
