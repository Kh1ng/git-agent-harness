mod approvals;
mod entry;
mod jsonl;

pub use self::approvals::active_paid_route_approval_destinations_from_entries;
#[allow(unused_imports)]
pub use self::entry::{
    review_generation, AttemptBehaviorMetrics, AttemptRecord, AttemptRoutingRecord, BehaviorMetric,
    BehaviorMetricQuality, FailureClass, FailureStage, LedgerEntry, LedgerUsage,
    RoutingCandidateDiagnostic, RoutingDiagnostics, CURRENT_REVIEW_CONTRACT_VERSION,
    LEDGER_SCHEMA_VERSION, REVIEW_CONTRACT_VERSION,
};
#[allow(unused_imports)]
pub use self::jsonl::{
    active_paid_route_approvals, active_paid_route_approvals_from_entries,
    active_review_hold_work_ids, active_review_hold_work_ids_from_entries, append,
    append_human_gate_if_transition, backfill_review_verdict, effective_human_gate_from_entries,
    effective_human_gate_from_index, entries_for_work_id, index_entries_by_work_id, is_entry_stale,
    read_entries, repair_truncated_tail, review_already_exists, work_id_aliases,
    EffectiveHumanGate, LedgerEntriesByWorkId, ReviewVerdictBackfill, TailRepair,
    REVIEW_HOLD_STALE_AFTER_HOURS,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::jsonl::{read_entries_call_count, reset_read_entries_call_count};

/// SQLite mirror of the JSONL ledger. `ledger.jsonl` remains the sole
/// source of truth (every read path in this file still reads it); this is
/// a redundant copy for evaluating SQLite as ledger storage without
/// committing to a migration yet -- see the module's `sync_from_jsonl` doc
/// for the tradeoff this makes.
#[path = "sqlite.rs"]
pub mod sqlite_store;

/// TICKET-072: append-only reconciliation of dispatched work with later
/// provider outcomes (MR merged, closed unmerged, state changed). A
/// separate log from `ledger.jsonl` -- never rewrites dispatch history,
/// only ever appends a new entry when a work item's classified state
/// actually changed since the last known reconciliation.
pub mod reconcile;

pub mod summary;
#[allow(unused_imports)]
pub use self::summary::{is_strong_model, usage_summary_for_backend, BackendUsageSummary, GroupBy};

#[cfg(test)]
pub(crate) mod test_util;
