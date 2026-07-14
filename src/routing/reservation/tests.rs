use super::super::decision::decide_with;
use super::ConcurrencyGuard;
use crate::config::tests::test_profile_for_notifications;
use crate::config::{Defaults, RoutingPolicy};
use crate::routing::RouteRequest;
use std::sync::Mutex;
use tempfile::TempDir;
use time::OffsetDateTime;

// These tests deliberately mutate the process-global live-slot counter.
// Keep them out of parallel test execution so one test cannot make the
// other's post-release assertion observe a still-held Claude slot.
static CONCURRENCY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn defaults() -> Defaults {
    Defaults {
        current_manager: None,
        artifact_root: String::new(),
        worktree_base: String::new(),
        llm_base_url: String::new(),
        llm_model_local: String::new(),
        llm_model_cloud: String::new(),
        routing: RoutingPolicy {
            default_backend: Some("codex".into()),
            weak_review_backend: Some("codex".into()),
            allow_review_fallback: true,
            ..RoutingPolicy::default()
        },
    }
}

fn profile() -> crate::config::Profile {
    let mut profile = test_profile_for_notifications();
    profile.routing.pm_backend = Some("claude".into());
    profile
}

fn path(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("availability.json")
}

fn backend_available(name: &str) -> bool {
    matches!(
        name,
        "claude" | "codex" | "openhands" | "agy" | "agy-main" | "agy-second" | "opencode"
    )
}

#[test]
fn preferred_backend_at_max_concurrent_falls_back() {
    let _lock = CONCURRENCY_TEST_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile
        .max_concurrent_per_model
        .insert("claude/".to_string(), 1);
    let _slot = ConcurrencyGuard::acquire("claude", None);

    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert!(decision.fallback_used);
    assert!(decision.routing_reason.contains("max_concurrent_reached"));
}

/// TICKET/issue (2026-07-11 hy3-free incident): reproduces the real bug
/// with real OS threads and the actual process-wide counter -- one
/// thread holds the only slot for a backend/model capped at
/// `max_concurrent=1` (standing in for an in-flight dispatch already
/// running against it), and a route decision made concurrently on a
/// second thread must skip that candidate and fall through to the next
/// one, exactly like the existing quota_exhausted/backend_outage skip
/// mechanics. Once the slot is released, routing picks the capped
/// backend again.
#[test]
fn concurrent_dispatch_holding_slot_forces_other_thread_to_fall_back() {
    let _lock = CONCURRENCY_TEST_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile
        .max_concurrent_per_model
        .insert("claude/".to_string(), 1);
    let state_path = path(&tmp);

    let (holder_ready_tx, holder_ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _slot = ConcurrencyGuard::acquire("claude", None);
        holder_ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    holder_ready_rx.recv().unwrap();

    let profile_for_decider = profile.clone();
    let state_path_for_decider = state_path.clone();
    let decider = std::thread::spawn(move || {
        decide_with(
            &defaults(),
            &profile_for_decider,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &state_path_for_decider,
            OffsetDateTime::now_utc(),
            backend_available,
        )
    });
    let decision_while_held = decider.join().unwrap().unwrap();
    release_tx.send(()).unwrap();
    holder.join().unwrap();

    assert_eq!(decision_while_held.effective_backend, "codex");
    assert!(decision_while_held
        .routing_reason
        .contains("max_concurrent_reached"));

    // Slot released -- the capped backend is eligible again.
    let decision_after_release = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &state_path,
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();
    assert_eq!(decision_after_release.effective_backend, "claude");
}
