use super::{work_id_aliases, LedgerEntry};
use std::collections::HashSet;

/// Instance-aware approval destinations. Historical approvals without an
/// explicit instance retain their legacy backend/model key; new grants target
/// the exact backend-instance/model destination.
pub fn active_paid_route_approval_destinations_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    work_id: &str,
) -> HashSet<(String, Option<String>)> {
    let mut active = HashSet::new();
    let aliases = work_id_aliases(work_id);
    for entry in entries {
        if entry.profile != profile_name
            || !entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
        {
            continue;
        }
        let identity = (
            entry
                .usage
                .backend_instance
                .clone()
                .unwrap_or_else(|| entry.effective_backend.clone()),
            entry.effective_model.clone(),
        );
        match entry.mode.as_str() {
            "paid_route_approval_grant" => {
                active.insert(identity);
            }
            "paid_route_approval_revoke" => {
                active.remove(&identity);
            }
            _ => {}
        }
    }
    active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_targets_one_backend_instance() {
        let profile = crate::ledger::test_util::profile();
        let grant = LedgerEntry::new_paid_route_approval_for_instance(
            "test",
            &profile,
            "ISSUE-42",
            "opencode",
            Some("opencode-api"),
            Some("openai/gpt-5"),
            true,
        );
        let active =
            active_paid_route_approval_destinations_from_entries(&[grant], "test", "ISSUE-42");

        assert!(active.contains(&("opencode-api".into(), Some("openai/gpt-5".into()))));
        assert!(!active.contains(&("opencode-subscription".into(), Some("openai/gpt-5".into()))));
    }
}
