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
