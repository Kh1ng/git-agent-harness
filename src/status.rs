use crate::availability;
use crate::config::GahConfig;
use crate::ledger::{self, LedgerEntry, RoutingDiagnostics};
use crate::sync;
use anyhow::Result;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct StatusSnapshot {
    pub schema_version: u32,
    pub generated_at: String,
    pub profile: ProfileIdentity,
    pub observations: Observations,
    pub merge_requests: Vec<sync::SyncMrJson>,
    pub availability: Vec<ScopeStatusJson>,
    pub recent_ledger: Option<RecentLedgerSummary>,
    pub constraints: Vec<Blocker>,
    /// Genuine profile-wide blockers (sync failure, infra unavailable, auth
    /// failure with no viable route, etc.) that halt ALL work. A
    /// ticket-scoped `human_required` ledger entry must NOT appear here --
    /// it is tracked per work item in `blocked_work_items` instead.
    pub blockers: Vec<Blocker>,
    /// Work items awaiting human action (e.g. a ticket/MR review verdict
    /// with `human_required`). Scoped to the work item(s) it affects; other
    /// eligible work remains dispatchable. Separated from `blockers` so
    /// `gah status` and the controller can distinguish a global freeze from
    /// a single blocked ticket.
    pub blocked_work_items: Vec<Blocker>,
    pub errors: Vec<StatusError>,
    /// TICKET-078: dispatch candidates from `docs/tickets/`, feeding
    /// `decide_next_action`'s DispatchTicket/Retry/Escalate rules.
    pub available_tickets: Vec<crate::models::AvailableTicket>,
    /// TICKET-118: fix attempt counts per branch for retry cap.
    pub fix_attempt_counts: std::collections::HashMap<String, usize>,
    /// TICKET-127: merge attempt counts per branch for the auto-merge
    /// retry cap.
    pub merge_attempt_counts: std::collections::HashMap<String, usize>,
    /// work_ids currently under an active `gah hold set` -- a manager
    /// session (human or supervising Claude/Codex/Hermes) reviewing the
    /// work_id's PR out of band. The controller must not auto-merge these
    /// out from under an in-progress review; see `decide_next_action`'s
    /// READY_FOR_HUMAN arm.
    pub review_held_work_ids: std::collections::HashSet<String>,
    /// TICKET-128: per-profile publishing policy. When PR/MR creation is
    /// disabled, the controller must never enter the auto-merge path even
    /// when a strong reviewer has approved and CI is green. This is an
    /// independent axis from reviewer routing and merge policy.
    pub publishing_allow_pr: bool,
    /// How many `gah loop` workers may run concurrently for this profile
    /// (see `Profile::max_parallel_workers`). Read by `gah-supervisor.sh`
    /// to decide how many worker loops to launch when not given explicitly
    /// on its own command line.
    pub max_parallel_workers: u32,
    /// TICKET-157: per-backend "configured for this profile" signal. Keyed
    /// by logical backend name. `true` means the backend has a real Rust
    /// implementation AND is set up for the active profile (an explicit
    /// path or profile marker is configured). This lets Settings distinguish
    /// "implemented but not set up for this profile" from "implemented and
    /// ready" rather than conflating "not explicitly marked unavailable" with
    /// "available". Backends with no implementation (e.g. grok/cursor) are
    /// simply absent from this map and should be reported as not_implemented
    /// by the frontend.
    pub backend_configured: std::collections::HashMap<String, bool>,
}

#[derive(Serialize)]
pub struct ProfileIdentity {
    pub profile: String,
    pub display_name: String,
    pub repo_id: String,
    pub provider: String,
    pub local_path: String,
    pub default_target_branch: String,
    /// Resolved per-repo merge policy (Issue #124 / TICKET-127). Inherits the
    /// canonical/defaults policy when the profile doesn't set its own.
    pub merge_policy: crate::config::MergePolicy,
}

#[derive(Serialize)]
pub struct Observations {
    pub sync: ObservationStatus,
    pub availability: ObservationStatus,
    pub ledger: ObservationStatus,
}

#[derive(Serialize)]
pub struct ObservationStatus {
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct ScopeStatusJson {
    pub backend: String,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    pub eligible_now: bool,
    pub reason: Option<String>,
    pub unavailable_until: Option<String>,
    pub source: Option<String>,
    pub last_error_summary: Option<String>,
    pub observed_at: Option<String>,
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct RecentLedgerSummary {
    pub most_recent_dispatch_timestamp: String,
    pub most_recent_effective_backend: String,
    pub most_recent_effective_model: Option<String>,
    pub most_recent_work_id: Option<String>,
    pub most_recent_mode: String,
    pub most_recent_validation_result: Option<String>,
    pub most_recent_failure_class: Option<String>,
    pub most_recent_failure_stage: Option<String>,
    pub most_recent_branch: Option<String>,
    pub most_recent_mr_url: Option<String>,
    pub attempts_started: Option<u32>,
    pub attempts_completed: Option<u32>,
    pub human_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_diagnostics: Option<RoutingDiagnostics>,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct Blocker {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct StatusError {
    pub subsystem: String,
    pub message: String,
    pub incomplete_snapshot: bool,
}

pub fn build_snapshot(
    cfg: &GahConfig,
    profile_name: &str,
    now: OffsetDateTime,
) -> Result<StatusSnapshot> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let generated_at = now.format(&Rfc3339).unwrap_or_default();

    let resolved_merge_policy = profile
        .effective_routing(&cfg.defaults)
        .merge_policy
        .unwrap_or_default();
    let profile_identity = ProfileIdentity {
        profile: profile_name.to_string(),
        display_name: profile.display_name.clone(),
        repo_id: profile.repo_id.clone(),
        provider: profile.provider.clone(),
        local_path: profile.local_path.clone(),
        default_target_branch: profile.default_target_branch.clone(),
        merge_policy: resolved_merge_policy,
    };

    let mut errors = Vec::new();

    // TICKET-118: Count fix attempts per branch from ledger entries
    let fix_attempt_counts = sync::count_fix_attempts_per_branch(cfg);
    // TICKET-127: Count merge attempts per branch for the auto-merge retry cap
    let merge_attempt_counts = sync::count_merge_attempts_per_branch(cfg);
    // review-hold: work_ids a manager session is actively reviewing via
    // `gah hold set`, out of band from gah's own auto-merge loop.
    let review_held_work_ids = ledger::active_review_hold_work_ids(cfg, profile_name);

    // 1. Sync State
    let mut merge_requests = Vec::new();
    let mut raw_mrs: Vec<sync::SyncMr> = Vec::new();
    let mut sync_obs = ObservationStatus { status: "ok" };

    // Ledger read is hoisted above the sync step so recently-merged MRs can be
    // enriched with their backend/model and review verdict (TICKET-198).
    let ledger_result = ledger::read_entries(cfg);
    let ledger_entries_by_work_id = ledger_result
        .as_ref()
        .map(|e| ledger::index_entries_by_work_id(e))
        .unwrap_or_default();
    match sync::fetch_mrs(profile) {
        Ok(mrs) => {
            let mrs = if profile.provider == "gitlab" {
                mrs.into_iter()
                    .filter(|mr| {
                        mr.state
                            .as_deref()
                            .is_some_and(|state| state.eq_ignore_ascii_case("opened"))
                            && !mr.merged
                    })
                    .collect()
            } else {
                mrs
            };
            merge_requests = mrs
                .iter()
                .map(|mr| sync::sync_mr_to_json(mr, None, &ledger_entries_by_work_id))
                .collect();
            raw_mrs = mrs;
        }
        Err(e) => {
            sync_obs.status = "error";
            errors.push(StatusError {
                subsystem: "sync".into(),
                message: format!("{:#}", e),
                incomplete_snapshot: true,
            });
        }
    }

    // 2. Availability State
    let state_path = availability::resolve_state_path();
    let mut availability_list = Vec::new();
    let mut avail_obs = ObservationStatus { status: "ok" };
    match availability::list_scopes(&state_path, now) {
        Ok(scopes) => {
            availability_list = scopes
                .into_iter()
                .map(|s| ScopeStatusJson {
                    backend: s.backend,
                    model: s.model,
                    quota_pool: s.quota_pool,
                    eligible_now: s.eligible,
                    reason: s.reason.map(|r| r.as_str().to_string()),
                    unavailable_until: s.unavailable_until,
                    source: s.source.map(|r| r.as_str().to_string()),
                    last_error_summary: s.last_error_summary,
                    observed_at: s.observed_at,
                    scope: s.scope.map(|s| match s {
                        availability::BlockScope::BackendWide => "backend_wide".into(),
                        availability::BlockScope::ModelSpecific => "model_specific".into(),
                        availability::BlockScope::QuotaPool => "quota_pool".into(),
                    }),
                })
                .collect();
        }
        Err(e) => {
            avail_obs.status = "error";
            errors.push(StatusError {
                subsystem: "availability".into(),
                message: format!("{:#}", e),
                incomplete_snapshot: true,
            });
        }
    }

    // 3. Ledger State
    let mut recent_ledger = None;
    let mut ledger_obs = ObservationStatus { status: "ok" };
    match &ledger_result {
        Ok(entries) => {
            let mut latest: Option<&LedgerEntry> = None;
            let mut max_ts: Option<OffsetDateTime> = None;

            for entry in entries.iter().filter(|e| e.profile == profile_name) {
                if let Ok(ts) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
                    match max_ts {
                        Some(m) if ts > m => {
                            max_ts = Some(ts);
                            latest = Some(entry);
                        }
                        None => {
                            max_ts = Some(ts);
                            latest = Some(entry);
                        }
                        _ => {}
                    }
                } else {
                    if latest.is_none() {
                        latest = Some(entry);
                    }
                }
            }

            if let Some(entry) = latest {
                recent_ledger = Some(RecentLedgerSummary {
                    most_recent_dispatch_timestamp: entry.timestamp.clone(),
                    most_recent_effective_backend: entry.effective_backend.clone(),
                    most_recent_effective_model: entry.effective_model.clone(),
                    most_recent_work_id: entry.work_id.clone(),
                    most_recent_mode: entry.mode.clone(),
                    most_recent_validation_result: entry.validation_result.clone(),
                    most_recent_failure_class: entry.failure_class.clone(),
                    most_recent_failure_stage: entry.failure_stage.clone(),
                    most_recent_branch: entry.branch.clone(),
                    most_recent_mr_url: entry.mr_url.clone(),
                    attempts_started: Some(entry.attempts_started),
                    attempts_completed: Some(entry.attempts_completed),
                    human_required: entry.human_required,
                    routing_diagnostics: entry.routing_diagnostics.clone(),
                });
            }
        }
        Err(e) => {
            ledger_obs.status = "error";
            errors.push(StatusError {
                subsystem: "ledger".into(),
                message: format!("{:#}", e),
                incomplete_snapshot: true,
            });
        }
    }

    // 4. Blockers and Constraints
    let mut constraints = Vec::new();
    let blockers = Vec::new();

    for avail in &availability_list {
        if !avail.eligible_now {
            constraints.push(Blocker {
                kind: "backend_unavailable".into(),
                reason: avail.reason.clone(),
                message: None,
                backend: Some(avail.backend.clone()),
                model: avail.model.clone(),
                until: avail.unavailable_until.clone(),
                source_reference: None,
            });
        }
    }

    // Removed all_backends_unavailable blocker check. Status has no routing context (mode),
    // so it correctly falls back to emitting individual availability constraints only.

    // TICKET-human-required-scoping: a ticket/MR review verdict with
    // `human_required = true` is WORK-ITEM SCOPED, never profile-wide. A
    // single blocked ticket must not freeze unrelated work. Derive the
    // effective `human_required` for each available ticket from its own
    // ledger history (canonical helper `ledger_lookup_for_ticket`) and
    // record it against that work item only. The newest ledger entry no
    // longer implicitly defines a profile-wide `human_required` blocker.
    // Genuine profile-wide blockers (sync failure, infra unavailable, auth
    // failure with no viable route) are still emitted above via
    // `availability`-derived constraints, NOT here.
    let mut blocked_work_items: Vec<Blocker> = Vec::new();

    // 5. Available tickets (TICKET-078): reuses the already-fetched `raw_mrs`
    // rather than calling sync::fetch_mrs a second time.
    let available_tickets =
        crate::dispatch::scan_available_tickets(profile, &raw_mrs, &ledger_entries_by_work_id);

    // TICKET-human-required-scoping: after the per-ticket human_required is
    // derived (in scan_available_tickets via ledger_lookup_for_ticket), record
    // each blocked work item in `blocked_work_items` so it stays visible in
    // status output without freezing the whole profile. Tickets whose
    // human_required state has since cleared are no longer shown as blocked.
    for ticket in &available_tickets {
        if ticket.human_required {
            blocked_work_items.push(Blocker {
                kind: "human_required".into(),
                reason: Some("ledger_human_required".into()),
                message: Some("Ledger indicates human intervention required".into()),
                backend: None,
                model: None,
                until: None,
                source_reference: ticket.work_id.clone(),
            });
        }
    }

    // Project retry-cap-blocked MRs into blocked_work_items. An MR classified
    // NEEDS_FIX whose fix_attempt_counts >= AUTO_RETRY_CAP will be returned as
    // HumanRequired by decide_next_action, but this is a controller decision
    // not a ledger human_required flag — without this projection, gah status
    // shows no blockers while the supervisor pings human_required every cycle.
    const AUTO_RETRY_CAP: usize = 2;
    for mr in &merge_requests {
        if matches!(mr.classification.as_str(), "CI_FAILED" | "NEEDS_FIX") {
            let attempts = fix_attempt_counts.get(&mr.branch).copied().unwrap_or(0);
            if attempts >= AUTO_RETRY_CAP {
                blocked_work_items.push(Blocker {
                    kind: "human_required".into(),
                    reason: Some("fix_retry_cap_exceeded".into()),
                    message: Some(format!(
                        "MR on branch '{}' classified {} but fix retry cap ({}) exceeded",
                        mr.branch, mr.classification, AUTO_RETRY_CAP
                    )),
                    backend: None,
                    model: None,
                    until: None,
                    source_reference: Some(mr.branch.clone()),
                });
            }
        }
    }

    // TICKET-157: build the per-backend "configured for this profile" signal.
    // Only backends with a real Rust implementation are listed. `configured`
    // is true when the profile sets up this backend (an explicit executable
    // path or profile marker). `configured_path` echoes the configured marker
    // for display. Backends with no implementation (grok/cursor) are omitted
    // entirely so the frontend can show them as not_implemented.
    let implemented_backends = [
        "codex",
        "claude",
        "agy",
        "agy-main",
        "agy-second",
        "vibe",
        "opencode",
        "openhands",
    ];
    let mut backend_configured: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    for backend in implemented_backends {
        let configured = profile.is_backend_configured(backend);
        backend_configured.insert(backend.to_string(), configured);
    }

    let snapshot = StatusSnapshot {
        schema_version: 1,
        generated_at,
        profile: profile_identity,
        observations: Observations {
            sync: sync_obs,
            availability: avail_obs,
            ledger: ledger_obs,
        },
        merge_requests,
        availability: availability_list,
        recent_ledger,
        constraints,
        blockers,
        blocked_work_items,
        errors,
        available_tickets,
        fix_attempt_counts,
        merge_attempt_counts,
        review_held_work_ids,
        publishing_allow_pr: profile.publishing.allow_pull_request_creation,
        max_parallel_workers: profile.max_parallel_workers(),
        backend_configured,
    };

    Ok(snapshot)
}

pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let snapshot = build_snapshot(cfg, profile_name, now)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        println!("Status for Profile: {}", profile_name);
        println!(
            "Observations: Sync={}, Availability={}, Ledger={}",
            snapshot.observations.sync.status,
            snapshot.observations.availability.status,
            snapshot.observations.ledger.status
        );

        if snapshot.blockers.is_empty() {
            println!("Blockers: None");
        } else {
            println!("Blockers:");
            for b in &snapshot.blockers {
                println!(
                    "  - {}: {}",
                    b.kind,
                    b.message
                        .as_deref()
                        .unwrap_or(b.reason.as_deref().unwrap_or("unknown"))
                );
            }
        }

        if !snapshot.constraints.is_empty() {
            println!("Constraints:");
            for c in &snapshot.constraints {
                println!(
                    "  - {}: {}",
                    c.backend.as_deref().unwrap_or(""),
                    c.reason.as_deref().unwrap_or("unknown")
                );
            }
        }

        if !snapshot.errors.is_empty() {
            println!("Errors:");
            for e in &snapshot.errors {
                println!("  - [{}] {}", e.subsystem, e.message);
            }
        }

        if let Some(ledger) = &snapshot.recent_ledger {
            if let Some(diag) = &ledger.routing_diagnostics {
                println!("Recent Routing:");
                if let Some(summary) = &diag.human_summary {
                    println!("  {}", summary);
                }
                for candidate in &diag.candidates {
                    let mut line = format!(
                        "  - {}",
                        match &candidate.model {
                            Some(model) => format!("{}/{}", candidate.backend, model),
                            None => candidate.backend.clone(),
                        }
                    );
                    if let Some(pool) = &candidate.quota_pool {
                        line.push_str(&format!(" pool={pool}"));
                    }
                    if let Some(pace) = &candidate.pace_band {
                        line.push_str(&format!(" pace={pace}"));
                    }
                    if let Some(cost_class) = &candidate.cost_class {
                        line.push_str(&format!(" cost={cost_class}"));
                    }
                    if let Some(skip_reason) = &candidate.skip_reason {
                        line.push_str(&format!(" skipped={skip_reason}"));
                    }
                    println!("{line}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::{AvailabilityRecord, AvailabilityState, Reason, Source, Status};
    use crate::ledger::{LedgerEntry, RoutingCandidateDiagnostic, RoutingDiagnostics};
    use std::fs;
    use tempfile::TempDir;

    fn make_test_cfg(tmp: &TempDir) -> GahConfig {
        let path = tmp.path().join("cfg.toml");
        fs::write(
            &path,
            r#"
[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
        )
        .unwrap();
        crate::config::load(Some(path.to_str().unwrap())).unwrap()
    }

    #[test]
    fn empty_clean_profile_snapshot() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        // Force availability and ledger to be read from temp
        std::env::set_var("GAH_AVAILABILITY_PATH", tmp.path().join("avail.json"));
        std::env::set_var("GAH_LEDGER_PATH", tmp.path().join("ledger.jsonl"));

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(snap.schema_version, 1);
        assert_eq!(snap.profile.profile, "test");
        assert_eq!(snap.observations.ledger.status, "ok");
        assert_eq!(snap.observations.availability.status, "ok");
        assert!(snap.merge_requests.is_empty());
        assert!(snap.availability.is_empty());
        assert!(snap.recent_ledger.is_none());
        assert!(snap.blockers.is_empty());
        assert!(snap.constraints.is_empty());
    }

    #[test]
    fn active_backend_wide_block() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let avail_path = tmp.path().join("avail.json");
        std::env::set_var("GAH_AVAILABILITY_PATH", &avail_path);

        let state = AvailabilityState {
            version: 1,
            records: vec![AvailabilityRecord {
                backend: "claude".into(),
                model: None,
                quota_pool: None,
                status: Status::Unavailable,
                reason: Reason::RateLimited,
                observed_at: "2026-07-04T00:00:00Z".into(),
                unavailable_until: Some("2099-01-01T00:00:00Z".into()),
                source: Source::BackendError,
                last_error_summary: None,
            }],
        };
        fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(snap.availability.len(), 1);
        assert_eq!(
            snap.availability[0].observed_at.as_deref().unwrap(),
            "2026-07-04T00:00:00Z"
        );
        assert_eq!(snap.constraints.len(), 1);
        assert_eq!(snap.constraints[0].kind, "backend_unavailable");
        assert_eq!(snap.constraints[0].backend.as_deref().unwrap(), "claude");
    }

    #[test]
    fn model_specific_availability_block_preserves_scope() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let avail_path = tmp.path().join("avail.json");
        std::env::set_var("GAH_AVAILABILITY_PATH", &avail_path);

        let state = AvailabilityState {
            version: 1,
            records: vec![AvailabilityRecord {
                backend: "claude".into(),
                model: Some("claude-3-5".into()),
                quota_pool: None,
                status: Status::Unavailable,
                reason: Reason::RateLimited,
                observed_at: "2026-07-04T00:00:00Z".into(),
                unavailable_until: Some("2099-01-01T00:00:00Z".into()),
                source: Source::BackendError,
                last_error_summary: None,
            }],
        };
        fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(
            snap.availability[0].scope.as_deref().unwrap(),
            "model_specific"
        );
        assert_eq!(snap.availability[0].model.as_deref().unwrap(), "claude-3-5");
        assert_eq!(snap.constraints[0].model.as_deref().unwrap(), "claude-3-5");
    }

    #[test]
    fn expired_availability_record_skipped() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let avail_path = tmp.path().join("avail.json");
        std::env::set_var("GAH_AVAILABILITY_PATH", &avail_path);

        let state = AvailabilityState {
            version: 1,
            records: vec![AvailabilityRecord {
                backend: "claude".into(),
                model: None,
                quota_pool: None,
                status: Status::Unavailable,
                reason: Reason::RateLimited,
                observed_at: "2026-07-04T00:00:00Z".into(),
                unavailable_until: Some("2020-01-01T00:00:00Z".into()), // Past
                source: Source::BackendError,
                last_error_summary: None,
            }],
        };
        fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(snap.availability.len(), 1);
        assert!(snap.availability[0].eligible_now);
        assert!(snap.constraints.is_empty());
    }

    #[test]
    fn human_required_state_becomes_blocker() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let ledger_path = tmp.path().join("ledger.jsonl");
        std::env::set_var("GAH_LEDGER_PATH", &ledger_path);

        let mut entry = LedgerEntry::new(
            "test",
            &cfg.profiles["test"],
            "test",
            "test",
            "test",
            None,
            None,
        );
        entry.human_required = true;
        entry.timestamp = "2026-07-04T00:00:00Z".into();
        fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert!(snap.recent_ledger.unwrap().human_required);
        // TICKET-human-required-scoping: an unassociated historical entry is
        // informational only; blockers are emitted only for current work.
        assert!(snap.blockers.is_empty());
        assert!(snap.blocked_work_items.is_empty());
    }

    #[test]
    fn partial_subsystem_error_is_in_errors() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let avail_path = tmp.path().join("avail.json");
        std::env::set_var("GAH_AVAILABILITY_PATH", &avail_path);

        // Write garbage JSON to force parsing error
        fs::write(&avail_path, "{garbage").unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(snap.observations.availability.status, "error");

        let avail_error = snap
            .errors
            .iter()
            .find(|e| e.subsystem == "availability")
            .unwrap();
        assert!(avail_error.incomplete_snapshot);
    }

    #[test]
    fn ledger_failure_and_attempt_fields_are_populated() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let ledger_path = tmp.path().join("ledger.jsonl");
        std::env::set_var("GAH_LEDGER_PATH", &ledger_path);

        let mut entry = LedgerEntry::new(
            "test",
            &cfg.profiles["test"],
            "test",
            "test",
            "test",
            None,
            None,
        );
        entry.failure_class = Some("backend_error".into());
        entry.failure_stage = Some("agent_run".into());
        entry.attempts_started = 3;
        entry.attempts_completed = 2;
        entry.timestamp = "2026-07-04T00:00:00Z".into();
        fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        let summary = snap.recent_ledger.unwrap();
        assert_eq!(
            summary.most_recent_failure_class.as_deref(),
            Some("backend_error")
        );
        assert_eq!(
            summary.most_recent_failure_stage.as_deref(),
            Some("agent_run")
        );
        assert_eq!(summary.attempts_started, Some(3));
        assert_eq!(summary.attempts_completed, Some(2));
    }

    #[test]
    fn recent_ledger_exposes_work_id() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let ledger_path = tmp.path().join("ledger.jsonl");
        std::env::set_var("GAH_LEDGER_PATH", &ledger_path);

        let mut entry = LedgerEntry::new(
            "test",
            &cfg.profiles["test"],
            "codex",
            "fix",
            "docs/tickets/TICKET-095-ledger-work-identity.md",
            None,
            None,
        );
        entry.work_id = Some("TICKET-095".into());
        entry.timestamp = "2026-07-04T00:00:00Z".into();
        fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

        let now = OffsetDateTime::now_utc();
        let snap = build_snapshot(&cfg, "test", now).unwrap();

        assert_eq!(
            snap.recent_ledger.unwrap().most_recent_work_id.as_deref(),
            Some("TICKET-095")
        );
    }

    #[test]
    fn recent_ledger_exposes_routing_diagnostics() {
        let _lock = crate::test_support::LEDGER_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let ledger_path = tmp.path().join("ledger.jsonl");
        std::env::set_var("GAH_LEDGER_PATH", &ledger_path);

        let mut entry = LedgerEntry::new(
            "test",
            &cfg.profiles["test"],
            "codex",
            "fix",
            "test",
            None,
            None,
        );
        entry.timestamp = "2026-07-04T00:00:00Z".into();
        entry.routing_diagnostics = Some(RoutingDiagnostics {
            policy_reordered_candidates: true,
            selected_backend: Some("codex".into()),
            selected_model: Some("gpt-5.4".into()),
            selected_quota_pool: Some("codex-main".into()),
            selected_pace_band: Some("aggressive_burn".into()),
            selected_cost_class: Some("included_quota".into()),
            selected_over: vec!["openhands/gpt-5.4 (paid $0.2500)".into()],
            candidates: vec![RoutingCandidateDiagnostic {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                default_order: Some(1),
                consideration_order: Some(0),
                pace_band: Some("aggressive_burn".into()),
                cost_class: Some("included_quota".into()),
                skip_reason: None,
                unavailable_until: None,
            }],
            human_summary: Some("selected codex/gpt-5.4".into()),
        });
        fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

        let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();
        let diagnostics = snap.recent_ledger.unwrap().routing_diagnostics.unwrap();
        assert!(diagnostics.policy_reordered_candidates);
        assert_eq!(
            diagnostics.selected_quota_pool.as_deref(),
            Some("codex-main")
        );
    }

    #[test]
    fn mr_classification_and_recommended_action_stable() {
        let mr = sync::SyncMr {
            title: "Test PR".into(),
            body: None,
            branch: "gah/test-branch".into(),
            labels: vec!["gah-ready-for-human".into()],
            url: Some("https://github.com/owner/repo/pull/1".into()),
            id: Some("1".into()),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: Some("CLEAN".into()),
            merged: false,
            updated_at: None,
            merged_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: None,
        };
        let class = sync::classify(&mr);
        assert_eq!(class, "READY_FOR_HUMAN");
        let action = sync::RecommendedAction::from_class(class);
        assert_eq!(action, sync::RecommendedAction::HumanMergeDecision);
    }

    #[test]
    fn mr_closed_unmerged_is_terminal_in_snapshot() {
        let mr = sync::SyncMr {
            title: "Closed PR".into(),
            body: None,
            branch: "gah/closed-branch".into(),
            labels: vec!["gah-human-review".into()],
            url: Some("https://github.com/owner/repo/pull/2".into()),
            id: Some("2".into()),
            state: Some("closed".into()),
            draft: true,
            merge_status: Some("DIRTY".into()),
            merged: false,
            updated_at: None,
            merged_at: None,
            ci_failed: true,
            ci_passed: false,
            ci_pending: false,
            work_id: None,
        };
        let class = sync::classify(&mr);
        assert_eq!(class, "CLOSED_UNMERGED");
        let action = sync::RecommendedAction::from_class(class);
        assert_eq!(action, sync::RecommendedAction::None);
    }
}
