use crate::ledger::LedgerEntry;

pub(super) fn canonicalize_review_ledger_identity(
    ledger: &mut LedgerEntry,
    source_branch: &str,
    mr_url: Option<&str>,
    mr_title: Option<&str>,
) {
    ledger.branch = Some(source_branch.to_string());
    ledger.mr_url = mr_url.map(str::to_string);
    if ledger.work_id.is_none() {
        ledger.work_id = mr_title.and_then(crate::sync::extract_work_id_from_title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_mr_identity_is_canonicalized_to_provider_branch_url_and_work() {
        let profile = crate::ledger::test_util::profile();
        let mut ledger = LedgerEntry::new("real", &profile, "claude", "review", "mr:7", None, None);
        ledger.branch = Some("mr:7".into());

        canonicalize_review_ledger_identity(
            &mut ledger,
            "feature/review",
            Some("https://gitlab.example.test/owner/repo/-/merge_requests/7"),
            Some("Draft: [GAH] Fix: #701 metadata identity"),
        );

        assert_eq!(ledger.branch.as_deref(), Some("feature/review"));
        assert_eq!(
            ledger.mr_url.as_deref(),
            Some("https://gitlab.example.test/owner/repo/-/merge_requests/7")
        );
        assert_eq!(ledger.work_id.as_deref(), Some("#701"));
    }

    #[test]
    fn explicit_controller_work_identity_is_not_replaced_by_title_inference() {
        let profile = crate::ledger::test_util::profile();
        let mut ledger = LedgerEntry::new("real", &profile, "claude", "review", "mr:7", None, None);
        ledger.work_id = Some("#700".into());

        canonicalize_review_ledger_identity(
            &mut ledger,
            "feature/review",
            Some("https://example.test/mr/7"),
            Some("Draft: [GAH] Fix: #701 metadata identity"),
        );

        assert_eq!(ledger.work_id.as_deref(), Some("#700"));
    }
}
