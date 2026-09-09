use crate::config::GahConfig;
use crate::ledger::LedgerEntry;

pub(super) fn is_policy_approval_gate(entry: &LedgerEntry) -> bool {
    entry.human_required
        && entry.human_required_reason_code.as_deref()
            == Some(crate::controller::HumanRequiredReason::PolicyApproval.as_str())
        && entry.failure_class.as_deref()
            == Some(crate::ledger::FailureClass::HumanBlocked.as_str())
}

pub(super) fn append_ledger_entry(
    cfg: &GahConfig,
    ledger: &LedgerEntry,
    policy_approval_gate: bool,
) -> anyhow::Result<bool> {
    if policy_approval_gate {
        let appended = crate::ledger::append_human_gate_if_transition(cfg, ledger)?;
        if appended {
            return Ok(true);
        }
    }
    crate::ledger::append(cfg, ledger).map(|_| false)
}

pub(super) fn should_notify_dispatch_failure(error: &anyhow::Error) -> bool {
    if super::review_budget_exhausted_error(error).is_some()
        || super::capacity_deferred_error(error)
    {
        return false;
    }
    // Approval skips have their own candidate-scoped notice. Ordinary quota
    // and authentication backpressure needs no terminal failure alert.
    !error
        .downcast_ref::<crate::routing::RouteError>()
        .is_some_and(|route| {
            let skipped = match route {
                crate::routing::RouteError::ApprovalRequired { .. } => return true,
                crate::routing::RouteError::NoEligibleBackend { skipped, .. } => skipped,
            };
            skipped
                .iter()
                .any(|skip| skip.reason == "operator_approval_required")
                || (!skipped.is_empty()
                    && skipped.iter().all(|skip| {
                        skip.reason.contains("quota_exhausted")
                            || skip.reason.contains("authentication_error")
                    }))
        })
}
