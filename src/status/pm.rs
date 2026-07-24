use crate::config::Profile;
use crate::ledger::LedgerEntry;
use serde::Serialize;

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct PmParentStatus {
    pub work_id: String,
    pub source_issue_number: String,
    pub plan_fingerprint: String,
    pub child_issue_numbers: Vec<String>,
    pub open_child_count: usize,
    pub completed: bool,
    pub reconciled: bool,
}

pub(super) fn project(
    profile: &Profile,
    profile_name: &str,
    entries: &[LedgerEntry],
) -> (
    Vec<PmParentStatus>,
    std::collections::HashMap<String, usize>,
    Option<String>,
) {
    let mut publications = std::collections::BTreeMap::<String, &LedgerEntry>::new();
    let mut attempts = std::collections::HashMap::<String, usize>::new();
    let mut reconciled = std::collections::HashSet::<(String, String)>::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.profile == profile_name && entry.repo_id == profile.repo_id)
    {
        if matches!(
            entry.mode.as_str(),
            "clear_attempts"
                | "external_approval_request"
                | "external_approval_grant"
                | "external_approval_consume"
                | "external_approval_revoke"
                | "external_approval_expire"
                | "external_approval_deny"
        ) {
            if let Some(work_id) = entry.work_id.as_ref() {
                attempts.remove(work_id);
            }
            continue;
        }
        if entry.mode == "pm_publish" && entry.pm_publication_status.as_deref() == Some("published")
        {
            if let (Some(work_id), Some(_)) =
                (entry.work_id.as_ref(), entry.pm_plan_fingerprint.as_ref())
            {
                publications.insert(work_id.clone(), entry);
            }
        }
        if matches!(entry.mode.as_str(), "pm" | "pm_orchestration") && entry.failure_class.is_some()
        {
            if let Some(work_id) = entry.work_id.as_ref() {
                *attempts.entry(work_id.clone()).or_default() += 1;
            }
        }
        if entry.mode == "pm_reconcile"
            && entry.pm_publication_status.as_deref() == Some("completed")
        {
            if let (Some(work_id), Some(fingerprint)) =
                (entry.work_id.as_ref(), entry.pm_plan_fingerprint.as_ref())
            {
                reconciled.insert((work_id.clone(), fingerprint.clone()));
            }
        }
    }

    let needs_provider_snapshot = publications
        .values()
        .any(|entry| !entry.pm_child_issue_numbers.is_empty());
    let provider_snapshot = needs_provider_snapshot
        .then(|| crate::provider::list_provider_issues(profile))
        .transpose();
    let (issues_by_number, mut error) = match provider_snapshot {
        Ok(Some(issues)) => (
            issues
                .into_iter()
                .map(|issue| (issue.number.clone(), issue))
                .collect::<std::collections::HashMap<_, _>>(),
            None,
        ),
        Ok(None) => (std::collections::HashMap::new(), None),
        Err(provider_error) => (
            std::collections::HashMap::new(),
            Some(format!(
                "reading provider child state for PM reconciliation: {provider_error:#}"
            )),
        ),
    };

    let mut missing = Vec::new();
    let mut states = Vec::new();
    for (work_id, entry) in publications {
        let fingerprint = entry.pm_plan_fingerprint.clone().unwrap_or_default();
        let mut open_child_count = 0;
        for number in &entry.pm_child_issue_numbers {
            match issues_by_number.get(number) {
                Some(issue)
                    if matches!(
                        issue.state.to_ascii_lowercase().as_str(),
                        "closed" | "merged"
                    ) => {}
                Some(_) => open_child_count += 1,
                None => {
                    open_child_count += 1;
                    if error.is_none() {
                        missing.push(number.clone());
                    }
                }
            }
        }
        states.push(PmParentStatus {
            work_id: work_id.clone(),
            source_issue_number: entry
                .source_issue_number
                .clone()
                .unwrap_or_else(|| work_id.trim_start_matches('#').to_string()),
            plan_fingerprint: fingerprint.clone(),
            child_issue_numbers: entry.pm_child_issue_numbers.clone(),
            open_child_count,
            completed: open_child_count == 0,
            reconciled: reconciled.contains(&(work_id, fingerprint)),
        });
    }
    if error.is_none() && !missing.is_empty() {
        missing.sort();
        missing.dedup();
        error = Some(format!(
            "provider snapshot omitted PM child issue(s): {}",
            missing.join(", ")
        ));
    }
    (states, attempts, error)
}

#[cfg(test)]
mod tests {
    use super::project;

    #[test]
    fn clear_attempts_resets_pm_failure_budget() {
        let profile = crate::ledger::test_util::profile();
        let mut failed = crate::ledger::LedgerEntry::new(
            "real",
            &profile,
            "control-plane",
            "pm_orchestration",
            "#561",
            None,
            None,
        );
        failed.work_id = Some("#561".into());
        failed.set_failure(
            crate::ledger::FailureClass::HarnessError,
            crate::ledger::FailureStage::Sync,
        );
        let clear = crate::ledger::LedgerEntry::new_clear_attempts("real", &profile, "#561");

        let (_, counts, _) = project(&profile, "real", &[failed.clone(), clear, failed]);

        assert_eq!(counts.get("#561"), Some(&1));
    }
}
