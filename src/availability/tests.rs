use super::*;
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

fn path(tmp: &TempDir) -> PathBuf {
    tmp.path().join("availability.json")
}

#[allow(clippy::too_many_arguments)]
fn record_unavailable(
    state_path: &Path,
    b: &str,
    m: Option<&str>,
    r: Reason,
    s: Source,
    u: Option<OffsetDateTime>,
    e: Option<String>,
    now: OffsetDateTime,
) -> Result<()> {
    super::record_unavailable(state_path, b, m, None, r, s, u, e, now)
}

fn record_available(
    state_path: &Path,
    b: &str,
    m: Option<&str>,
    s: Source,
    now: OffsetDateTime,
) -> Result<()> {
    super::record_available(state_path, b, m, None, s, now)
}

fn availability_for(
    state_path: &Path,
    b: &str,
    m: Option<&str>,
    now: OffsetDateTime,
) -> Result<AvailabilityDecision> {
    super::availability_for(state_path, b, m, None, now)
}

#[test]
fn missing_state_file_means_eligible() {
    let tmp = TempDir::new().unwrap();
    let decision =
        availability_for(&path(&tmp), "claude", None, OffsetDateTime::now_utc()).unwrap();
    assert!(decision.eligible);
}

#[test]
fn backend_wide_block_blocks_all_models() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        Some("quota exhausted".into()),
        now,
    )
    .unwrap();

    let d1 = availability_for(&p, "claude", None, now).unwrap();
    assert!(!d1.eligible);
    assert_eq!(d1.scope, Some(BlockScope::BackendWide));

    let d2 = availability_for(&p, "claude", Some("claude-sonnet-4"), now).unwrap();
    assert!(!d2.eligible);
    assert_eq!(d2.scope, Some(BlockScope::BackendWide));
}

#[test]
fn model_specific_block_blocks_only_that_model() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "openhands",
        Some("litellm_proxy/deepseek-v4"),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::minutes(30)),
        None,
        now,
    )
    .unwrap();

    let blocked =
        availability_for(&p, "openhands", Some("litellm_proxy/deepseek-v4"), now).unwrap();
    assert!(!blocked.eligible);
    assert_eq!(blocked.scope, Some(BlockScope::ModelSpecific));

    let other_model = availability_for(&p, "openhands", Some("litellm_proxy/other"), now).unwrap();
    assert!(other_model.eligible);

    let backend_wide_query = availability_for(&p, "openhands", None, now).unwrap();
    assert!(
        backend_wide_query.eligible,
        "a model-specific block must not block the backend-wide (no model) query"
    );
}

#[test]
fn expired_temporary_record_becomes_eligible() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let observed_at = OffsetDateTime::now_utc() - time::Duration::hours(2);
    record_unavailable(
        &p,
        "codex",
        None,
        Reason::RateLimited,
        Source::BackendError,
        Some(observed_at + time::Duration::hours(1)), // expired an hour ago
        None,
        observed_at,
    )
    .unwrap();

    let decision = availability_for(&p, "codex", None, OffsetDateTime::now_utc()).unwrap();
    assert!(decision.eligible);
}

#[test]
fn manual_disable_without_expiry_remains_blocked() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let observed_at = OffsetDateTime::now_utc() - time::Duration::days(30);
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::ManualDisable,
        Source::Manual,
        None,
        Some("disabled by operator".into()),
        observed_at,
    )
    .unwrap();

    // Even long after it was recorded, with no expiry it is still blocked.
    let decision = availability_for(&p, "claude", None, OffsetDateTime::now_utc()).unwrap();
    assert!(!decision.eligible);
    assert_eq!(decision.reason, Some(Reason::ManualDisable));
}

#[test]
fn explicit_available_record_clears_manual_disable() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let t0 = OffsetDateTime::now_utc() - time::Duration::hours(1);
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::ManualDisable,
        Source::Manual,
        None,
        None,
        t0,
    )
    .unwrap();
    record_available(
        &p,
        "claude",
        None,
        Source::Manual,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let decision = availability_for(&p, "claude", None, OffsetDateTime::now_utc()).unwrap();
    assert!(decision.eligible);
}

#[test]
fn backend_wide_block_takes_precedence_over_model_availability() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    // Model itself was explicitly marked available...
    record_available(&p, "claude", Some("claude-sonnet-4"), Source::Manual, now).unwrap();
    // ...but the whole backend is down.
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::BackendOutage,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        None,
        now,
    )
    .unwrap();

    let decision = availability_for(&p, "claude", Some("claude-sonnet-4"), now).unwrap();
    assert!(!decision.eligible);
    assert_eq!(decision.scope, Some(BlockScope::BackendWide));
}

#[test]
fn two_sequential_updates_preserve_both_records() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::QuotaExhausted,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();
    record_unavailable(
        &p,
        "codex",
        None,
        Reason::RateLimited,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();

    let state = load_state(&p).unwrap();
    assert_eq!(state.records.len(), 2);
    assert!(!availability_for(&p, "claude", None, now).unwrap().eligible);
    assert!(!availability_for(&p, "codex", None, now).unwrap().eligible);
}

#[test]
fn concurrent_updates_do_not_lose_records() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let backends = ["claude", "codex", "openhands", "b4", "b5", "b6", "b7", "b8"];
    let barrier = Arc::new(Barrier::new(backends.len()));
    let handles: Vec<_> = backends
        .into_iter()
        .map(|backend| {
            let p = p.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                record_unavailable(
                    &p,
                    backend,
                    None,
                    Reason::Unknown,
                    Source::BackendError,
                    None,
                    None,
                    OffsetDateTime::now_utc(),
                )
                .unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let state = load_state(&p).unwrap();
    assert_eq!(
        state.records.len(),
        8,
        "concurrent appends must not clobber each other"
    );
}

#[test]
fn malformed_state_file_returns_actionable_error_and_is_not_overwritten() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    fs::write(&p, "not json at all").unwrap();

    let err = load_state(&p).unwrap_err();
    assert!(format!("{:#}", err).contains("parsing availability state"));

    let update_err = update_state(&p, |_| {}).unwrap_err();
    assert!(format!("{:#}", update_err).contains("parsing availability state"));

    // The bad file must still be there, untouched.
    assert_eq!(fs::read_to_string(&p).unwrap(), "not json at all");
}

#[test]
fn unsupported_schema_version_fails_clearly() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    fs::write(&p, r#"{"version":99,"records":[]}"#).unwrap();

    let err = load_state(&p).unwrap_err();
    assert!(format!("{:#}", err).contains("unsupported schema version"));
}

#[test]
fn atomic_write_leaves_valid_json() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    record_unavailable(
        &p,
        "claude",
        None,
        Reason::Unknown,
        Source::Manual,
        None,
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let text = fs::read_to_string(&p).unwrap();
    let _: AvailabilityState = serde_json::from_str(&text).unwrap();
    // No leftover temp files.
    let leftover: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftover.is_empty());
}

#[test]
fn xdg_state_home_path_resolution() {
    let resolved = resolve_state_path_from_env(Some("/custom/xdg-state"), Some("/home/user"));
    assert_eq!(
        resolved,
        PathBuf::from("/custom/xdg-state/gah/availability.json")
    );
}

#[test]
fn fallback_local_state_path_resolution() {
    let resolved = resolve_state_path_from_env(None, Some("/home/user"));
    assert_eq!(
        resolved,
        PathBuf::from("/home/user/.local/state/gah/availability.json")
    );
}

#[test]
fn empty_xdg_state_home_falls_back_like_unset() {
    let resolved = resolve_state_path_from_env(Some(""), Some("/home/user"));
    assert_eq!(
        resolved,
        PathBuf::from("/home/user/.local/state/gah/availability.json")
    );
}

// ── TICKET-069: reason/source display strings match the wire format ────

#[test]
fn reason_as_str_matches_serde_snake_case_wire_format() {
    for reason in [
        Reason::RateLimited,
        Reason::QuotaExhausted,
        Reason::AuthenticationError,
        Reason::BackendOutage,
        Reason::ManualDisable,
        Reason::Unknown,
    ] {
        let wire = serde_json::to_string(&reason).unwrap();
        assert_eq!(wire, format!("\"{}\"", reason.as_str()));
    }
}

#[test]
fn source_as_str_matches_serde_snake_case_wire_format() {
    for source in [Source::BackendError, Source::Manual, Source::Imported] {
        let wire = serde_json::to_string(&source).unwrap();
        assert_eq!(wire, format!("\"{}\"", source.as_str()));
    }
}

// ── TICKET-069: list_scopes / format_remaining ──────────────────────────

#[test]
fn list_scopes_is_empty_for_missing_state_file() {
    let tmp = TempDir::new().unwrap();
    let scopes = list_scopes(&path(&tmp), OffsetDateTime::now_utc()).unwrap();
    assert!(scopes.is_empty());
}

#[test]
fn list_scopes_covers_backend_wide_and_model_specific_rows() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "openhands",
        None,
        Reason::BackendOutage,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        Some("outage".into()),
        now,
    )
    .unwrap();
    record_unavailable(
        &p,
        "openhands",
        Some("litellm_proxy/deepseek-v4"),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::minutes(5)),
        None,
        now,
    )
    .unwrap();
    record_available(&p, "codex", None, Source::Manual, now).unwrap();

    let scopes = list_scopes(&p, now).unwrap();
    assert_eq!(scopes.len(), 3);

    let openhands_backend = scopes
        .iter()
        .find(|s| s.backend == "openhands" && s.model.is_none())
        .unwrap();
    assert!(!openhands_backend.eligible);
    assert_eq!(openhands_backend.reason, Some(Reason::BackendOutage));
    assert_eq!(openhands_backend.source, Some(Source::BackendError));
    assert_eq!(
        openhands_backend.last_error_summary.as_deref(),
        Some("outage")
    );

    let openhands_model = scopes
        .iter()
        .find(|s| s.backend == "openhands" && s.model.is_some())
        .unwrap();
    // Backend-wide outage takes precedence, so this row is also
    // ineligible, and its *reported* reason is the backend-wide one --
    // the model-specific rate-limit is masked, same as availability_for.
    assert!(!openhands_model.eligible);
    assert_eq!(openhands_model.reason, Some(Reason::BackendOutage));

    let codex = scopes.iter().find(|s| s.backend == "codex").unwrap();
    assert!(codex.eligible);
}

#[test]
fn format_remaining_renders_hours_and_minutes() {
    let now = OffsetDateTime::now_utc();
    let until = now_rfc3339(now + time::Duration::minutes(134)); // 2h 14m
    assert_eq!(format_remaining(&until, now).as_deref(), Some("2h 14m"));
}

#[test]
fn format_remaining_renders_minutes_only_under_an_hour() {
    let now = OffsetDateTime::now_utc();
    let until = now_rfc3339(now + time::Duration::minutes(9));
    assert_eq!(format_remaining(&until, now).as_deref(), Some("9m"));
}

#[test]
fn format_remaining_returns_none_for_a_past_timestamp() {
    let now = OffsetDateTime::now_utc();
    let until = now_rfc3339(now - time::Duration::minutes(5));
    assert_eq!(format_remaining(&until, now), None);
}

#[test]
fn quota_pool_blocks_associated_candidates() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();

    // 1. Initially it should be eligible
    let d = super::availability_for(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        now,
    )
    .unwrap();
    assert!(d.eligible);

    // 2. Mark the pool unavailable
    super::record_unavailable(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        None,
        now,
    )
    .unwrap();

    // 3. Now the candidate with that pool is blocked
    let d1 = super::availability_for(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        now,
    )
    .unwrap();
    assert!(!d1.eligible);
    assert_eq!(d1.scope, Some(BlockScope::QuotaPool));

    // 4. A different candidate sharing the pool is also blocked!
    let d2 = super::availability_for(&p, "claude", Some("claude-haiku"), Some("claude-main"), now)
        .unwrap();
    assert!(!d2.eligible);
    assert_eq!(d2.scope, Some(BlockScope::QuotaPool));

    // 5. A candidate NOT sharing the pool is eligible
    let d3 = super::availability_for(
        &p,
        "claude",
        Some("claude-haiku"),
        Some("claude-other"),
        now,
    )
    .unwrap();
    assert!(d3.eligible);

    // 6. Clearing the pool with record_available
    super::record_available(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        Source::Manual,
        now,
    )
    .unwrap();
    let d4 = super::availability_for(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        now,
    )
    .unwrap();
    assert!(d4.eligible);
}

#[test]
fn cli_clear_marks_backend_and_model_available_without_touching_others() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "codex",
        Some("gpt-5.4-mini"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(5)),
        Some("quota exhausted".into()),
        now,
    )
    .unwrap();
    record_unavailable(
        &p,
        "vibe",
        Some("default"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(5)),
        Some("unrelated".into()),
        now,
    )
    .unwrap();

    // Pre-condition: codex model is blocked.
    assert!(
        !availability_for(&p, "codex", Some("gpt-5.4-mini"), now)
            .unwrap()
            .eligible
    );

    cli::clear(&p, "codex", Some("gpt-5.4-mini"), None).unwrap();

    let d_codex = availability_for(&p, "codex", Some("gpt-5.4-mini"), now).unwrap();
    assert!(d_codex.eligible, "cleared scope must be eligible again");
    let d_vibe = availability_for(&p, "vibe", Some("default"), now).unwrap();
    assert!(!d_vibe.eligible, "clear must not touch other backends");

    // Append-only: the original blocking record is preserved, with a
    // newer manual `available` record now winning the scope.
    let state = load_state(&p).unwrap();
    assert!(state.records.iter().any(|r| r.backend == "codex"
        && r.model.as_deref() == Some("gpt-5.4-mini")
        && r.status == Status::Unavailable));
    assert!(state.records.iter().any(|r| r.backend == "codex"
        && r.model.as_deref() == Some("gpt-5.4-mini")
        && r.status == Status::Available
        && r.source == Source::Manual));
}

#[test]
fn cli_clear_without_model_marks_backend_wide_available() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &p,
        "agy",
        Some("Gemini 3.5 Flash (Medium)"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(5)),
        None,
        now,
    )
    .unwrap();
    record_unavailable(
        &p,
        "agy",
        Some("Claude Sonnet 4.6 (Thinking)"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(5)),
        None,
        now,
    )
    .unwrap();

    cli::clear(&p, "agy", None, None).unwrap();
    assert!(
        availability_for(&p, "agy", Some("Gemini 3.5 Flash (Medium)"), now)
            .unwrap()
            .eligible
    );
    assert!(
        availability_for(&p, "agy", Some("Claude Sonnet 4.6 (Thinking)"), now)
            .unwrap()
            .eligible
    );
}

#[test]
fn derive_quota_pool_separates_pools_per_account_and_ignores_non_agy() {
    let d = |b, m| super::derive_quota_pool(b, Some(m)).unwrap();
    assert_eq!(d("agy", "Gemini 3.5 Flash (Medium)"), "agy:google-native");
    assert_eq!(d("agy-main", "Gemini 3.5 Flash"), "agy:google-native");
    assert_eq!(d("agy", "gemini-2.0-flash"), "agy:google-native");
    assert_eq!(d("agy", "Claude Sonnet 4.6 (Thinking)"), "agy:external");
    assert_eq!(
        d("agy-second", "Gemini 3.1 Pro (High)"),
        "agy-second:google-native"
    );
    assert_eq!(d("agy-second", "Claude Sonnet 4.6"), "agy-second:external");
    assert_eq!(d("agy-third", "Gemini 3.5"), "agy-third:google-native");
    assert_eq!(super::derive_quota_pool("claude", Some("Gemini 3.5")), None);
    assert_eq!(super::derive_quota_pool("agy", None), None);
    assert_eq!(super::derive_quota_pool("agy", Some("   ")), None);
    assert_eq!(d("agy", "OpenGeminiWrapper"), "agy:external");
    let r = |b, m, p| super::resolve_candidate_quota_pool(b, m, p).unwrap();
    let g = "agy:google-native";
    assert_eq!(r("agy", Some("Gemini 3.5"), Some("agy")), g);
    assert_eq!(r("agy-main", Some("Gemini 3.5"), Some("agy-main")), g);
    assert_eq!(r("agy", Some("Gemini 3.5"), None), g);
    // A fully-qualified pool is operator intent and is respected verbatim.
    assert_eq!(
        r("agy", Some("Gemini 3.5"), Some("agy:external")),
        "agy:external"
    );
    assert_eq!(
        r("codex", Some("Gemini 3.5"), Some("agy-custom")),
        "agy-custom"
    );
    assert_eq!(
        r("agy", Some("Gemini 3.5"), Some("agy-custom")),
        "agy-custom"
    );
    assert_eq!(r("agy", None, Some("agy")), "agy");
}

#[test]
fn agy_pool_isolation_blocks_only_same_pool_on_same_account() {
    // Issue #180: live rate-limit must not spill onto external pool of same account.
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    let ag = super::derive_quota_pool("agy", Some("Gemini 3.5 Flash (Medium)")).unwrap();
    let ae = super::derive_quota_pool("agy", Some("Claude Sonnet 4.6 (Thinking)")).unwrap();
    super::record_unavailable(
        &p,
        "agy",
        Some("Gemini 3.5 Flash (Medium)"),
        Some(&ag),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        None,
        now,
    )
    .unwrap();
    super::record_available(
        &p,
        "agy",
        Some("Claude Sonnet 4.6 (Thinking)"),
        Some(&ae),
        Source::Manual,
        now,
    )
    .unwrap();

    // Gemini on `agy` is blocked; Claude on `agy` is not.
    let gemini =
        super::availability_for(&p, "agy", Some("Gemini 3.5 Flash (Medium)"), Some(&ag), now)
            .unwrap();
    assert!(!gemini.eligible);
    assert_eq!(gemini.scope, Some(BlockScope::QuotaPool));
    let claude = super::availability_for(
        &p,
        "agy",
        Some("Claude Sonnet 4.6 (Thinking)"),
        Some(&ae),
        now,
    )
    .unwrap();
    assert!(
        claude.eligible,
        "a google-native block must not spill onto the external pool of the same account"
    );
}

#[test]
fn cli_clear_with_quota_pool_marks_only_that_pool() {
    let tmp = TempDir::new().unwrap();
    let p = path(&tmp);
    let now = OffsetDateTime::now_utc();
    super::record_unavailable(
        &p,
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        None,
        now,
    )
    .unwrap();
    assert!(
        !super::availability_for(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            now
        )
        .unwrap()
        .eligible
    );

    cli::clear(&p, "claude", None, Some("claude-main")).unwrap();

    assert!(
        super::availability_for(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            now
        )
        .unwrap()
        .eligible,
        "clearing the pool must unblock candidates sharing it"
    );

    // Append-only: exactly one new manual record added.
    let state = load_state(&p).unwrap();
    let manual: Vec<_> = state
        .records
        .iter()
        .filter(|r| r.source == Source::Manual && r.status == Status::Available)
        .collect();
    assert_eq!(manual.len(), 1);
    assert_eq!(manual[0].quota_pool.as_deref(), Some("claude-main"));
}
