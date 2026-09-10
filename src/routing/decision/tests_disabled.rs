//! Issue #822: routing skips disabled backend instances. Split from
//! `tests.rs` to stay under the source-size guard.

use super::super::test_support::*;
use super::*;
use tempfile::TempDir;

/// Issue #822: a disabled backend instance stays declared (its identity is
/// still resolvable for status/attribution) but routing must skip it with a
/// typed reason and fall through to the next candidate.
#[test]
fn candidate_list_skips_disabled_instances_with_typed_reason() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.backend_instances.insert(
        "codex-paid".into(),
        crate::config::BackendInstanceConfig {
            runner_kind: "codex".into(),
            logical_backend: Some("codex".into()),
            executable: Some("/bin/sh".into()),
            enabled: false,
            ..Default::default()
        },
    );
    let mut disabled = candidate_config("codex", Some("gpt-5.4-mini"), None);
    disabled.instance = Some("codex-paid".into());
    profile.routing.pm_candidates = Some(vec![
        disabled,
        candidate_config("claude", Some("claude-sonnet"), None),
    ]);

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
            exact_route_required: false,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    let routing_diagnostics = decision.routing_diagnostics.expect("skips recorded");
    let skipped_codex_paid = routing_diagnostics
        .candidates
        .iter()
        .find(|candidate| candidate.backend_instance.as_deref() == Some("codex-paid"))
        .expect("disabled candidate recorded in diagnostics");
    assert!(
        skipped_codex_paid.skip_reason.is_some(),
        "disabled candidate must be skipped, got: {:?}",
        skipped_codex_paid
    );
    assert_eq!(
        skipped_codex_paid.skip_reason.as_deref(),
        Some("backend instance disabled"),
        "the skip reason must name the disabled state"
    );
}
