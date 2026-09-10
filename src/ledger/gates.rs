//! Pure gate/alias/index analysis over ledger entries (#519 follow-up:
//! split from `jsonl.rs` to stay under the source-size guard). No
//! filesystem or locking here — callers feed entries in.

use super::LedgerEntry;
use crate::job_kind::JobKind;
use std::collections::BTreeMap;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub type LedgerEntriesByWorkId = BTreeMap<String, Vec<LedgerEntry>>;

/// The current work-item-scoped human gate derived from append-only ledger
/// history. Consumers must use this shared projection instead of inferring the
/// state from whichever tickets happened to survive issue discovery: an open
/// PR/MR remains gated even when its source issue is dependency-blocked,
/// closed, or otherwise absent from the dispatchable ticket list.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveHumanGate {
    pub reason_code: Option<String>,
    pub dispatch_reason: Option<String>,
    pub message: Option<String>,
    pub mode: String,
    pub timestamp: String,
    pub routing_diagnostics: Option<super::entry::RoutingDiagnostics>,
    pub review_contract_version: Option<u32>,
    pub review_generation: Option<String>,
}

/// Helper to identify review-derived human gates (stuck-loop gates, review-evidence gates,
/// review budget exhaustion, post-review policy approvals, etc.) distinct from explicit operator holds
/// and non-review implementation failure caps.
pub fn is_review_derived_gate(entry: &LedgerEntry) -> bool {
    let reason_code = entry.human_required_reason_code.as_deref();
    let dispatch_reason = entry.dispatch_reason.as_deref();
    let is_review = JobKind::parse(&entry.mode) == Ok(JobKind::Review);

    if is_review {
        return true;
    }

    if matches!(
        reason_code,
        Some(
            "review_evidence_gate"
                | "review_output_invalid_exhausted"
                | "review_ceiling_exhausted"
                | "fix_retry_cap_exceeded"
        )
    ) {
        return true;
    }

    // Stuck-loop detection applies to every controller action, including an
    // initial implementation dispatch. It is review-derived only when the
    // controller attached the exact review generation of an MR lifecycle
    // action. Treating every legacy/non-MR stuck gate as review-derived would
    // make a contract bump erase unrelated durable safety holds and would
    // defeat append_human_gate_if_transition's idempotency.
    if (reason_code == Some("stuck_loop_gate") || dispatch_reason == Some("stuck_loop_gate"))
        && entry.review_generation.is_some()
    {
        return true;
    }

    if entry.validation_result.as_deref() == Some("review_budget_exhausted")
        || (reason_code == Some("retry_budget_exhausted")
            && dispatch_reason == Some("post_review_repair"))
    {
        return true;
    }

    if reason_code == Some("policy_approval")
        && (is_review || dispatch_reason == Some("post_review_repair"))
    {
        return true;
    }

    false
}

const LEDGER_ENTRY_STALE_AFTER_DAYS: i64 = 14;

pub fn is_entry_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return false;
        }
    } else {
        return false;
    };
    OffsetDateTime::now_utc() - entry_time > time::Duration::days(LEDGER_ENTRY_STALE_AFTER_DAYS)
}

/// Resolve the effective human gate for one work item using the same
/// transition semantics as ticket discovery. A completed review may clear its
/// own provisional hold; review no-ops and unrelated completions may not clear
/// a hold. Paid-route grants stay in the history so status can verify that the
/// grant matches the exact blocked route before releasing it. `clear-attempts`
/// remains the unconditional release transition. Control-only records never
/// create a gate.
pub fn effective_human_gate_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    effective_human_gate_for_scope(entries, Some(profile_name), repo_id, work_id)
}

fn effective_human_gate_for_scope(
    entries: &[LedgerEntry],
    profile_name: Option<&str>,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    let aliases = work_id_aliases(work_id);
    let mut gate = None;
    for entry in entries.iter().filter(|entry| {
        profile_name.is_none_or(|profile_name| entry.profile == profile_name)
            && entry.repo_id == repo_id
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
            && !is_entry_stale(entry)
    }) {
        match entry.mode.as_str() {
            "clear_attempts" => {
                gate = None;
                continue;
            }
            "paid_route_approval_grant" => {
                // Modern policy gates carry exact route diagnostics and must
                // be released only after status verifies this grant against
                // that route. Preserve legacy behavior for pre-reason-code
                // handoffs, whose requested identity cannot be reconstructed.
                if gate
                    .as_ref()
                    .and_then(|gate: &EffectiveHumanGate| gate.reason_code.as_deref())
                    != Some("policy_approval")
                {
                    gate = None;
                }
                continue;
            }
            "claim"
            | "external_approval_grant"
            | "paid_route_approval_revoke"
            | "external_approval_request"
            | "external_approval_consume"
            | "external_approval_revoke"
            | "external_approval_expire"
            | "external_approval_deny"
            | "review_hold"
            | "review_hold_release" => {
                continue;
            }
            _ if entry.validation_result.as_deref() == Some("deferred_capacity") => continue,
            "review" if !entry.human_required => {
                // Only a real terminal review supersedes a prior review gate.
                // Duplicate/no-op and failed publication records carry no
                // verdict and must not erase a pending policy approval.
                if entry.review_verdict.is_some() && entry.failure_class.is_none() {
                    gate = None;
                }
                continue;
            }
            _ if !entry.human_required => continue,
            _ => {}
        }

        // TICKET-711: Review-derived gates created under a superseded review contract version
        // (review_contract_version < CURRENT_REVIEW_CONTRACT_VERSION) are superseded and cannot
        // block a fresh attempt.
        if is_review_derived_gate(entry)
            && entry.review_contract_version.unwrap_or(0)
                < super::entry::CURRENT_REVIEW_CONTRACT_VERSION
        {
            continue;
        }

        gate = Some(EffectiveHumanGate {
            reason_code: entry.human_required_reason_code.clone(),
            dispatch_reason: entry.dispatch_reason.clone(),
            message: entry.error_summary.clone(),
            mode: entry.mode.clone(),
            timestamp: entry.timestamp.clone(),
            routing_diagnostics: entry.routing_diagnostics.clone(),
            review_contract_version: entry.review_contract_version,
            review_generation: entry.review_generation.clone(),
        });
    }
    gate
}

pub fn effective_human_gate_from_index(
    entries: &LedgerEntriesByWorkId,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    entries
        .get(work_id)
        .and_then(|entries| effective_human_gate_for_scope(entries, None, repo_id, work_id))
}

/// Native tracker issues use their provider-visible `#123` identity. Older
/// GAH records used `TICKET-123`; retain that as a read alias so migrating to
/// the tracker identity never forks history or re-dispatches completed work.
pub fn work_id_aliases(work_id: &str) -> Vec<String> {
    let legacy_number = work_id.strip_prefix("TICKET-").and_then(|rest| {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty()).then_some(digits)
    });
    let issue_number = work_id
        .strip_prefix('#')
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()));

    match (legacy_number.as_deref(), issue_number) {
        (Some(number), _) => vec![work_id.to_string(), format!("#{number}")],
        (_, Some(number)) => vec![work_id.to_string(), format!("TICKET-{number}")],
        _ => vec![work_id.to_string()],
    }
}

pub fn index_entries_by_work_id(entries: &[LedgerEntry]) -> LedgerEntriesByWorkId {
    let mut index = BTreeMap::new();
    for entry in entries {
        if let Some(work_id) = entry.work_id.as_ref() {
            for alias in work_id_aliases(work_id) {
                index
                    .entry(alias)
                    .or_insert_with(Vec::new)
                    .push(entry.clone());
            }
        }
    }
    index
}
