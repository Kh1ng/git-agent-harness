use super::SyncMr;
use crate::ledger::{LedgerEntriesByWorkId, LedgerEntry};
use sha2::{Digest, Sha256};

/// Stable identity of the provider metadata a reviewer actually inspected.
/// Length-prefixing avoids ambiguous concatenation; the versioned prefix
/// permits canonicalization changes without treating old identities as equal.
pub(crate) fn review_metadata_fingerprint(
    source_sha: Option<&str>,
    title: Option<&str>,
    body: Option<&str>,
    draft: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gah-review-metadata-v1\0");
    // GAH creates GitLab MRs with this exact marker and `--ready` removes it.
    // Draft state is hashed separately, so the marker is not title content.
    let title = title
        .and_then(|value| value.strip_prefix("Draft: "))
        .or(title);
    for value in [
        source_sha.unwrap_or(""),
        title.unwrap_or(""),
        body.unwrap_or(""),
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update([u8::from(draft)]);
    format!("sha256:{:x}", hasher.finalize())
}

impl SyncMr {
    pub(super) fn review_metadata_fingerprint(&self) -> String {
        review_metadata_fingerprint(
            self.source_sha.as_deref(),
            Some(&self.title),
            self.body.as_deref(),
            self.draft,
        )
    }
}

pub(super) fn latest_review_for_mr<'a>(
    ledger: &'a LedgerEntriesByWorkId,
    mr: &SyncMr,
) -> Option<&'a LedgerEntry> {
    let entries = ledger.get(mr.work_id.as_ref()?)?;
    entries.iter().rev().find(|entry| {
        entry.branch.as_deref() == Some(mr.branch.as_str())
            && matches!(
                entry
                    .review_verdict
                    .as_deref()
                    .or(entry.validation_result.as_deref()),
                Some("APPROVE" | "NEEDS_FIX" | "REJECT" | "HUMAN_REVIEW" | "REVIEW_OUTPUT_INVALID")
            )
    })
}

pub(super) fn review_metadata_matches(latest: &LedgerEntry, mr: &SyncMr) -> bool {
    let recorded = latest.review_metadata_fingerprint.as_deref();
    if recorded == Some(mr.review_metadata_fingerprint().as_str()) {
        return true;
    }
    // Accept only GAH's one-way draft-to-ready lifecycle transition;
    // re-drafting still invalidates the review.
    !mr.draft
        && recorded
            == Some(
                review_metadata_fingerprint(
                    mr.source_sha.as_deref(),
                    Some(&mr.title),
                    mr.body.as_deref(),
                    true,
                )
                .as_str(),
            )
}

/// Return implementation attribution independently from the latest review.
/// A metadata-only re-review appends a review without a new implementation.
pub(super) fn ledger_info_for_mr(
    ledger: &LedgerEntriesByWorkId,
    mr: &SyncMr,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let Some(entries) = mr.work_id.as_ref().and_then(|id| ledger.get(id)) else {
        return (None, None, None, None);
    };
    let implementation = entries
        .iter()
        .rev()
        .find(|entry| {
            matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.mr_url.as_deref() == mr.url.as_deref()
                && !entry.effective_backend.is_empty()
        })
        .or_else(|| {
            entries.iter().rev().find(|entry| {
                matches!(entry.mode.as_str(), "fix" | "improve")
                    && entry.branch.as_deref() == Some(mr.branch.as_str())
                    && !entry.effective_backend.is_empty()
            })
        });
    let review = entries.iter().rev().find(|entry| {
        entry.branch.as_deref() == Some(mr.branch.as_str()) && entry.review_verdict.is_some()
    });
    (
        implementation.map(|entry| entry.effective_backend.clone()),
        implementation.and_then(|entry| entry.effective_model.clone()),
        review.and_then(|entry| entry.review_verdict.clone()),
        review.and_then(|entry| entry.review_gate_reason.clone()),
    )
}
