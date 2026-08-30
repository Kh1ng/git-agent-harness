use crate::config::Profile;
use crate::ledger::LedgerEntry;
use std::collections::HashSet;
use std::path::Path;

const MAX_PRIOR_ATTEMPTS: usize = 2;
const MAX_CONTEXT_BYTES: usize = 4_096;
const ERROR_SUMMARY_MAX_BYTES: usize = 700;
const VALIDATION_TAIL_MAX_BYTES: usize = 900;
const BLOCKING_FINDINGS_MAX_BYTES: usize = 1_000;
const BLOCKING_FINDING_MAX_BYTES: usize = 600;

fn append_to_prompt(prompt: &mut String, context: Option<&str>) {
    if let Some(context) = context {
        prompt.push_str("\n\n");
        prompt.push_str(context);
    }
}

pub(super) fn build_redispatch_task(
    profile: &Profile,
    wt: &Path,
    args: &super::DispatchArgs,
    target: &str,
    issue_details: Option<&super::issues::IssueDetails>,
) -> String {
    let mut task = super::prompts::build_task(profile, wt, &args.mode, target, issue_details);
    append_to_prompt(&mut task, args.prior_attempt_context.as_deref());
    task
}

pub(crate) fn prior_attempt_context(
    entries: &[LedgerEntry],
    profile_name: &str,
    profile: &Profile,
    work_id: &str,
) -> Option<String> {
    let aliases = crate::ledger::work_id_aliases(work_id);
    let matches_work_id = |entry: &LedgerEntry| {
        entry
            .work_id
            .as_deref()
            .is_some_and(|id| aliases.iter().any(|alias| alias == id))
    };
    let branches: HashSet<&str> = entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && matches_work_id(entry)
        })
        .filter_map(|entry| entry.branch.as_deref())
        .collect();
    let selected: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && (matches_work_id(entry)
                    || entry
                        .branch
                        .as_deref()
                        .is_some_and(|branch| branches.contains(branch)))
                && has_evidence(entry, profile)
        })
        .rev()
        .take(MAX_PRIOR_ATTEMPTS)
        .collect();
    if selected.is_empty() {
        return None;
    }

    let mut context = String::from("## Prior attempts\n");
    for (index, entry) in selected.into_iter().enumerate() {
        context.push_str(&format!("\n### Attempt {} (newest first)\n", index + 1));
        render_entry(&mut context, entry, profile);
    }
    let context = crate::redact::redact(&context);
    Some(cap_context(&context))
}

fn has_evidence(entry: &LedgerEntry, profile: &Profile) -> bool {
    entry.failure_class.is_some()
        || entry.failure_stage.is_some()
        || entry.error_summary.is_some()
        || !entry.review_blocking_findings.is_empty()
        || validation_failure_tail(entry, profile).is_some()
}

fn render_entry(context: &mut String, entry: &LedgerEntry, profile: &Profile) {
    let latest_attempt = entry.attempts.last();
    context.push_str(&format!(
        "- failure_class: {}\n- stage: {}\n",
        entry
            .failure_class
            .as_deref()
            .or_else(|| latest_attempt.and_then(|attempt| attempt.failure_class.as_deref()))
            .unwrap_or("unknown"),
        entry
            .failure_stage
            .as_deref()
            .or_else(|| latest_attempt.and_then(|attempt| attempt.failure_stage.as_deref()))
            .unwrap_or("unknown"),
    ));
    if let Some(summary) = entry.error_summary.as_deref() {
        append_text(context, "Error summary", summary, ERROR_SUMMARY_MAX_BYTES);
    }
    if let Some(validation) = validation_failure_tail(entry, profile) {
        context.push_str("\nFailing validation tail:\n");
        context.push_str(&crate::dispatch::prompts::indent_untrusted_text(
            &redacted_suffix(&validation, VALIDATION_TAIL_MAX_BYTES),
        ));
        context.push('\n');
    }
    if !entry.review_blocking_findings.is_empty() {
        context.push_str("\nReview blocking findings:\n");
        let mut findings = String::new();
        for finding in &entry.review_blocking_findings {
            findings.push_str("- ");
            findings.push_str(&redacted_prefix(finding, BLOCKING_FINDING_MAX_BYTES));
            findings.push('\n');
            if findings.len() >= BLOCKING_FINDINGS_MAX_BYTES {
                break;
            }
        }
        context.push_str(&crate::dispatch::prompts::indent_untrusted_text(
            crate::dispatch::utf8_safe_prefix(&findings, BLOCKING_FINDINGS_MAX_BYTES),
        ));
        context.push('\n');
    }
}

fn append_text(context: &mut String, label: &str, text: &str, max_bytes: usize) {
    context.push_str(&format!("\n{label}:\n"));
    context.push_str(&crate::dispatch::prompts::indent_untrusted_text(
        &redacted_prefix(text, max_bytes),
    ));
    context.push('\n');
}

fn redacted_prefix(text: &str, max_bytes: usize) -> String {
    let redacted = crate::redact::redact(text);
    crate::dispatch::utf8_safe_prefix(&redacted, max_bytes).to_string()
}

fn redacted_suffix(text: &str, max_bytes: usize) -> String {
    let redacted = crate::redact::redact(text);
    crate::dispatch::text::utf8_safe_suffix(&redacted, max_bytes).to_string()
}

fn cap_context(context: &str) -> String {
    const MARKER: &str = "\n[Prior-attempt context truncated]\n";
    if context.len() <= MAX_CONTEXT_BYTES {
        return context.to_string();
    }
    format!(
        "{}{}",
        crate::dispatch::utf8_safe_prefix(context, MAX_CONTEXT_BYTES - MARKER.len()),
        MARKER
    )
}

fn validation_failure_tail(entry: &LedgerEntry, profile: &Profile) -> Option<String> {
    let session_dir = Path::new(entry.session_dir.as_deref()?);
    if !session_dir.starts_with(Path::new(&profile.artifact_root)) {
        return None;
    }
    let read_attempt = |attempt_number| {
        std::fs::read_to_string(
            session_dir
                .join(format!("attempt-{attempt_number}"))
                .join("validation-failure.txt"),
        )
        .ok()
    };

    let failed = entry
        .attempts
        .iter()
        .rev()
        .filter(|attempt| attempt.validation_result.as_deref() == Some("failed"))
        .find_map(|attempt| read_attempt(attempt.attempt_number));
    if failed.is_some() || !entry.attempts.is_empty() {
        return failed;
    }
    read_attempt(entry.attempts_started?)
}

#[cfg(test)]
mod tests {
    use super::prior_attempt_context;
    use crate::ledger::{AttemptRecord, LedgerEntry};
    use std::fs;

    #[test]
    fn renders_prior_failure_and_validation_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();
        let session_dir = tmp.path().join("artifacts/sessions/prior-run");
        let attempt_dir = session_dir.join("attempt-1");
        fs::create_dir_all(&attempt_dir).unwrap();
        fs::write(
            attempt_dir.join("validation-failure.txt"),
            "$ cargo test retry_context\nfirst failure\nassertion failed: retained tail\n",
        )
        .unwrap();

        let mut entry = LedgerEntry::new(
            "test",
            &profile,
            "codex",
            "fix",
            "docs/tickets/TICKET-243.md",
            Some("prior-run".into()),
            Some(&session_dir),
        );
        entry.work_id = Some("TICKET-243".into());
        entry.failure_class = Some("validation_failure".into());
        entry.failure_stage = Some("post_validation".into());
        entry.error_summary = Some("validation did not pass".into());
        entry.attempts.push(AttemptRecord {
            attempt_number: 1,
            validation_result: Some("failed".into()),
            ..AttemptRecord::default()
        });

        let context = prior_attempt_context(&[entry], "test", &profile, "TICKET-243")
            .expect("prior failure should produce context");

        assert!(context.starts_with("## Prior attempts\n"));
        assert!(context.contains("failure_class: validation_failure"));
        assert!(context.contains("stage: post_validation"));
        assert!(context.contains("validation did not pass"));
        assert!(context.contains("$ cargo test retry_context"));
        assert!(context.contains("assertion failed: retained tail"));
    }

    #[test]
    fn omits_context_without_prior_evidence_for_work_item() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();
        let unrelated = LedgerEntry::new(
            "test",
            &profile,
            "codex",
            "fix",
            "other",
            Some("unrelated".into()),
            None,
        );

        assert!(prior_attempt_context(&[unrelated], "test", &profile, "TICKET-243").is_none());
    }

    #[test]
    fn keeps_only_two_latest_attempts_and_caps_redacted_review_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();

        let mut oldest = LedgerEntry::new(
            "test",
            &profile,
            "codex",
            "fix",
            "ticket",
            Some("oldest".into()),
            None,
        );
        oldest.work_id = Some("TICKET-243".into());
        oldest.branch = Some("gah/issue/gah-243".into());
        oldest.error_summary = Some("oldest attempt must be omitted".into());

        let mut second = oldest.clone();
        second.session_id = Some("second".into());
        second.error_summary = Some("second-most-recent failure".into());
        second.failure_class = Some("agent_failure".into());
        second.failure_stage = Some("agent_run".into());

        let mut review = LedgerEntry::new(
            "test",
            &profile,
            "claude",
            "review",
            "gah/issue/gah-243",
            Some("review".into()),
            None,
        );
        review.branch = Some("gah/issue/gah-243".into());
        review.review_blocking_findings = vec![format!(
            "review blocker includes ghp_abcdefghijklmnopqrstuvwxyz {} end-of-blocker",
            "x".repeat(8_000)
        )];

        let context =
            prior_attempt_context(&[oldest, second, review], "test", &profile, "TICKET-243")
                .expect("branch review evidence should produce context");

        assert!(context.contains("second-most-recent failure"));
        assert!(context.contains("Review blocking findings:"));
        assert!(context.contains("review blocker includes"));
        assert!(context.contains("[REDACTED:GITHUB_TOKEN]"));
        assert!(!context.contains("oldest attempt must be omitted"));
        assert!(!context.contains("end-of-blocker"));
        assert!(!context.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(
            context.len() <= 4_096,
            "context was {} bytes",
            context.len()
        );
    }

    #[test]
    fn redacts_evidence_before_cutoff_boundaries() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();
        let session_dir = tmp.path().join("artifacts/sessions/cutoff-run");
        let attempt_dir = session_dir.join("attempt-1");
        fs::create_dir_all(&attempt_dir).unwrap();
        fs::write(
            attempt_dir.join("validation-failure.txt"),
            format!(
                "{} {}{}",
                "v".repeat(99),
                "sk-abcdefghijklmnopqrstuvwxyz",
                "z".repeat(876)
            ),
        )
        .unwrap();

        let mut entry = LedgerEntry::new(
            "test",
            &profile,
            "codex",
            "fix",
            "ticket",
            Some("cutoff-run".into()),
            Some(&session_dir),
        );
        entry.work_id = Some("TICKET-243".into());
        entry.error_summary = Some(format!(
            "{} {}",
            "s".repeat(694),
            "ghp_abcdefghijklmnopqrstuvwxyz"
        ));
        entry.review_blocking_findings = vec![format!(
            "{} {}",
            "r".repeat(594),
            "glpat-ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        )];
        entry.attempts.push(AttemptRecord {
            attempt_number: 1,
            validation_result: Some("failed".into()),
            ..AttemptRecord::default()
        });

        let context = prior_attempt_context(&[entry], "test", &profile, "TICKET-243")
            .expect("cutoff evidence should produce context");

        assert!(!context.contains("ghp_a"), "summary leaked: {context}");
        assert!(!context.contains("glpat"), "review leaked: {context}");
        assert!(
            !context.contains("cdefghijklmnopqrstuvwxyz"),
            "validation leaked: {context}"
        );
        assert!(context.contains("[REDACTED:API_KEY]"));
    }

    #[test]
    fn uses_latest_failed_validation_artifact_before_not_run_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();
        let session_dir = tmp.path().join("artifacts/sessions/failed-then-not-run");
        let attempt_dir = session_dir.join("attempt-1");
        fs::create_dir_all(&attempt_dir).unwrap();
        fs::write(
            attempt_dir.join("validation-failure.txt"),
            "attempt one failed: assertion retained\n",
        )
        .unwrap();

        let mut entry = LedgerEntry::new(
            "test",
            &profile,
            "codex",
            "fix",
            "ticket",
            Some("failed-then-not-run".into()),
            Some(&session_dir),
        );
        entry.work_id = Some("TICKET-243".into());
        entry.attempts_started = Some(2);
        entry.attempts.push(AttemptRecord {
            attempt_number: 1,
            validation_result: Some("failed".into()),
            ..AttemptRecord::default()
        });
        entry.attempts.push(AttemptRecord {
            attempt_number: 2,
            validation_result: Some("not_run_backend_unavailable".into()),
            ..AttemptRecord::default()
        });

        let context = prior_attempt_context(&[entry], "test", &profile, "TICKET-243")
            .expect("failed validation artifact should produce context");

        assert!(context.contains("attempt one failed: assertion retained"));
    }
}
