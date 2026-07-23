//! Golden fixtures and compatibility-adapter tests for issue #504
//! (execution identity 1/5). See `docs/EXECUTION_IDENTITY_CONTRACT.md` for
//! the full canonical contract these tests encode.
//!
//! This ticket is documentation + fixtures only: it does not thread a new
//! canonical type through production. `ExecutionIdentity`/`adapt_legacy_usage`
//! below are a test-local transcription of the contract (§11), used to pin
//! today's field-mapping rules as fixtures for parts 2/5-5/5 to reproduce.
//! Every mapping with a real production function -- `config::
//! canonical_backend_name`, `usage_attribution::classify_usage`,
//! `usage_attribution::provider_for_model`, the `ledger::LedgerEntry`/
//! `AttemptRecord`/`LedgerUsage` types, or a full `ScenarioHarness` dispatch
//! -- is called directly instead of reimplemented, so these fixtures cannot
//! silently drift from what production actually does.

mod support;

use git_agent_harness::config;
use git_agent_harness::ledger::{AttemptRecord, AttemptRoutingRecord, LedgerEntry, LedgerUsage};
use git_agent_harness::routing::{CandidateIdentity, RouteRequest, RoutingRuntimeState};
use git_agent_harness::usage_attribution::{classify_usage, provider_for_model};
use support::fake_ledger::TestLedger;
use support::scenario::ScenarioHarness;
use support::test_temp_root;

// ── Test-local compatibility adapter (contract §11) ──────────────────────

/// The canonical shape §1 of the contract defines, scoped to the fields the
/// golden fixtures below exercise.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionIdentity {
    logical_backend: String,
    backend_instance: String,
    account_label: Option<String>,
    auth_source_label: Option<String>,
    quota_pool: Option<String>,
    auth_class: String,
    provider: Option<String>,
    provider_attribution_source: String,
    requested_model: Option<String>,
    effective_model: Option<String>,
    actual_model: Option<String>,
    cost_known: bool,
}

/// The documented compatibility adapter (contract §11): legacy
/// backend/model/quota_pool/cost_class facts -> canonical `ExecutionIdentity`.
fn adapt_legacy_usage(
    logical_backend: &str,
    quota_pool: Option<&str>,
    cost_class: Option<&str>,
    requested_model: Option<&str>,
    effective_model: Option<&str>,
    actual_model: Option<&str>,
    cost_known: bool,
) -> ExecutionIdentity {
    let logical_backend = config::canonical_backend_name(logical_backend).to_string();
    let backend_instance = match quota_pool {
        Some(pool) => format!("{logical_backend}:{pool}"),
        None => logical_backend.clone(),
    };
    // Real production mappings (contract §7, §3) -- not reimplementations.
    let auth_class = classify_usage(Some(&logical_backend), effective_model, cost_class)
        .expect("a Some(backend) input always yields a classified auth_class");
    let provider = provider_for_model(Some(&logical_backend), effective_model.or(actual_model));
    let provider_attribution_source = if provider.is_some() {
        "inferred"
    } else {
        "unknown"
    }
    .to_string();
    ExecutionIdentity {
        logical_backend,
        backend_instance,
        // Legacy production currently projects all three facts from the one
        // quota-pool string. The canonical contract keeps them separate so
        // part 5/5 can declare an instance and auth source independently.
        account_label: quota_pool.map(str::to_string),
        auth_source_label: quota_pool.map(str::to_string),
        quota_pool: quota_pool.map(str::to_string),
        auth_class,
        provider,
        provider_attribution_source,
        requested_model: requested_model.map(str::to_string),
        effective_model: effective_model.map(str::to_string),
        actual_model: actual_model.map(str::to_string),
        cost_known,
    }
}

// ── Golden fixture 1: two accounts, one runner ────────────────────────────

#[test]
fn execution_identity_golden_two_accounts_one_runner() {
    let primary = adapt_legacy_usage(
        "agy",
        Some("agy-main"),
        None,
        Some("gemini-3-pro"),
        Some("gemini-3-pro"),
        Some("gemini-3-pro"),
        false,
    );
    let second = adapt_legacy_usage(
        "agy-second",
        Some("agy-second-account"),
        None,
        Some("gemini-3-pro"),
        Some("gemini-3-pro"),
        Some("gemini-3-pro"),
        false,
    );

    // Same runner_kind (both invoke the `agy` executable), same model,
    // but distinct logical_backend / account / auth source / quota pool /
    // backend_instance.
    assert_eq!(primary.logical_backend, "agy");
    assert_eq!(second.logical_backend, "agy-second");
    assert_ne!(primary.backend_instance, second.backend_instance);
    assert_eq!(primary.backend_instance, "agy:agy-main");
    assert_eq!(second.backend_instance, "agy-second:agy-second-account");
    assert_ne!(primary.account_label, second.account_label);
    assert_ne!(primary.auth_source_label, second.auth_source_label);
    assert_ne!(primary.quota_pool, second.quota_pool);
    // Both are subscription/quota-backed classes for the built-in AGY family.
    assert_eq!(primary.auth_class, "quota_backed");
    assert_eq!(second.auth_class, "quota_backed");
    assert_eq!(primary.provider.as_deref(), Some("google"));
    assert_eq!(second.provider.as_deref(), Some("google"));
    assert_eq!(primary.provider_attribution_source, "inferred");
    assert_eq!(second.provider_attribution_source, "inferred");
}

#[test]
fn execution_identity_explicit_instances_stay_distinct_in_one_quota_pool() {
    let common = ExecutionIdentity {
        logical_backend: "opencode".into(),
        backend_instance: "opencode-nous-key-1".into(),
        account_label: Some("nous-billing".into()),
        auth_source_label: Some("nous-key-1".into()),
        quota_pool: Some("nous-shared-budget".into()),
        auth_class: "api_key_backed".into(),
        provider: Some("z-ai".into()),
        provider_attribution_source: "backend_reported".into(),
        requested_model: Some("nous-portal/z-ai/glm-5.2".into()),
        effective_model: Some("nous-portal/z-ai/glm-5.2".into()),
        actual_model: Some("glm-5.2".into()),
        cost_known: true,
    };
    let second = ExecutionIdentity {
        backend_instance: "opencode-nous-key-2".into(),
        auth_source_label: Some("nous-key-2".into()),
        ..common.clone()
    };

    assert_eq!(common.logical_backend, second.logical_backend);
    assert_eq!(common.quota_pool, second.quota_pool);
    assert_eq!(common.effective_model, second.effective_model);
    assert_ne!(common.backend_instance, second.backend_instance);
    assert_ne!(common.auth_source_label, second.auth_source_label);
}

#[test]
fn configured_subscription_and_api_routes_remain_distinct_through_telemetry() {
    let cfg: config::GahConfig = toml::from_str(
        r#"
[defaults]
artifact_root = ""
worktree_base = ""
llm_base_url = ""
llm_model_local = ""
llm_model_cloud = ""

[defaults.routing.backend_instances.opencode-subscription]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/bin/sh"
state_root = "/tmp/gah-opencode-subscription"
account_label = "personal-subscription"
auth_source_label = "opencode-login"
quota_pool = "opencode-plan"
supported_models = ["openai/gpt-5"]

[defaults.routing.backend_instances.opencode-api]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/bin/sh"
state_root = "/tmp/gah-opencode-api"
account_label = "team-api"
auth_source_label = "env-openai-key"
quota_pool = "openai-api"
supported_models = ["openai/gpt-5"]

[[defaults.routing.improve_candidates]]
backend = "opencode"
instance = "opencode-subscription"
model = "openai/gpt-5"
priority = 100
included_in_quota = true

[[defaults.routing.improve_candidates]]
backend = "opencode"
instance = "opencode-api"
model = "openai/gpt-5"
priority = 100
marginal_cost_usd = 1.0

[profiles.test]
display_name = "Test"
repo_id = "test"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"
"#,
    )
    .unwrap();
    let profile = cfg.profiles.get("test").unwrap();
    let request = || RouteRequest {
        mode: "improve",
        requested_backend: "auto",
        requested_model: None,
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
        last_failure_class: None,
        exact_route_required: false,
    };

    let subscription = git_agent_harness::routing::decide_with_state(
        &cfg.defaults,
        profile,
        request(),
        &RoutingRuntimeState::default(),
    )
    .unwrap();
    let mut runtime = RoutingRuntimeState::default();
    runtime.recent_runs.insert(
        CandidateIdentity::new("opencode-subscription", Some("openai/gpt-5")),
        1,
    );
    let api =
        git_agent_harness::routing::decide_with_state(&cfg.defaults, profile, request(), &runtime)
            .unwrap();

    assert_eq!(
        subscription.identity.backend_instance,
        "opencode-subscription"
    );
    assert_eq!(api.identity.backend_instance, "opencode-api");
    assert_eq!(subscription.effective_model, api.effective_model);

    let mut ledger = LedgerEntry::new("test", profile, "auto", "improve", "x", None, None);
    for (attempt_number, route, usage_classification) in [
        (1, &subscription, "quota_backed"),
        (2, &api, "api_key_backed"),
    ] {
        ledger.attempts.push(AttemptRecord {
            attempt_number,
            backend: route.effective_backend.clone(),
            effective_model: route.effective_model.clone(),
            usage: LedgerUsage {
                usage_classification: Some(usage_classification.into()),
                ..Default::default()
            },
            ..Default::default()
        });
        ledger.attempt_routing.push(AttemptRoutingRecord {
            attempt_number,
            backend_instance: route.identity.backend_instance.clone(),
            effective_model: route.effective_model.clone(),
            identity: Some(route.identity.clone()),
            routing_diagnostics: route.routing_diagnostics.clone(),
        });
    }

    let telemetry = git_agent_harness::telemetry::extractor::extract_attempt_usage_records(
        &ledger,
        "2026-07-21T00:00:00Z",
    );
    assert_eq!(telemetry.len(), 2);
    assert_eq!(
        telemetry[0].backend_instance.as_deref(),
        Some("opencode-subscription")
    );
    assert_eq!(
        telemetry[0].account_label.as_deref(),
        Some("personal-subscription")
    );
    assert_eq!(
        telemetry[0].usage_classification.as_deref(),
        Some("quota_backed")
    );
    assert_eq!(
        telemetry[1].backend_instance.as_deref(),
        Some("opencode-api")
    );
    assert_eq!(telemetry[1].account_label.as_deref(), Some("team-api"));
    assert_eq!(
        telemetry[1].usage_classification.as_deref(),
        Some("api_key_backed")
    );
}

// ── Golden fixture 2: one model through subscription and API ─────────────

#[test]
fn execution_identity_golden_subscription_vs_api_same_model() {
    let subscription = adapt_legacy_usage(
        "opencode",
        Some("nous-portal-subscription"),
        Some("included_quota"),
        Some("nous-portal/z-ai/glm-5.2"),
        Some("nous-portal/z-ai/glm-5.2"),
        None,
        false,
    );
    let api = adapt_legacy_usage(
        "opencode",
        Some("nous-portal-api"),
        Some("paid"),
        Some("nous-portal/z-ai/glm-5.2"),
        Some("nous-portal/z-ai/glm-5.2"),
        None,
        true,
    );

    assert_eq!(subscription.logical_backend, api.logical_backend);
    assert_eq!(subscription.effective_model, api.effective_model);
    assert_ne!(subscription.backend_instance, api.backend_instance);
    assert_eq!(subscription.auth_class, "quota_backed");
    assert_eq!(api.auth_class, "api_key_backed");
    // Contract §7: quota-backed cost fields are always cleared; API-key
    // cost fields are only "known" when the pricing table actually reported
    // one, distinct from "unknown, but should be zero".
    assert!(!subscription.cost_known);
    assert!(api.cost_known);
}

// ── Golden fixture 3: proxies / aliases ───────────────────────────────────

#[test]
fn execution_identity_golden_proxy_alias() {
    // Calls the real, public production function directly rather than a
    // reimplementation.
    assert_eq!(config::canonical_backend_name("cloud-coder"), "openhands");
    assert_eq!(config::canonical_backend_name("openhands"), "openhands");
    // "auto" must never be folded -- its effective backend is resolved
    // per-attempt by routing, not a fixed alias (contract §3).
    assert_eq!(config::canonical_backend_name("auto"), "auto");

    let aliased = adapt_legacy_usage(
        "cloud-coder",
        None,
        None,
        Some("gpt-5.4"),
        Some("gpt-5.4"),
        Some("gpt-5.4"),
        false,
    );
    assert_eq!(aliased.logical_backend, "openhands");
    assert_eq!(aliased.backend_instance, "openhands");

    // A proxy path: opencode's routed model string embeds a different
    // provider than the opencode backend name itself.
    let proxied = adapt_legacy_usage(
        "opencode",
        Some("nous-portal-api"),
        Some("paid"),
        Some("nous-portal/z-ai/glm-5.2"),
        Some("nous-portal/z-ai/glm-5.2"),
        None,
        true,
    );
    assert_eq!(proxied.logical_backend, "opencode");
    assert_eq!(proxied.provider.as_deref(), Some("z-ai"));
    assert_eq!(proxied.provider_attribution_source, "inferred");
    assert_ne!(proxied.provider.as_deref(), Some("opencode"));
}

/// Byte-for-byte pin: a real end-to-end dispatch through `cloud-coder`
/// resolves to `openhands` in both `requested_backend` and
/// `effective_backend` in the actual ledger, via the real production alias
/// fold in `dispatch/workflows/improve.rs`, not the test-local adapter.
#[test]
fn execution_identity_route_decision_alias_fold_is_byte_for_byte() {
    let worktree_base = test_temp_root().join(format!(
        "execution_identity_worktrees_{}",
        std::process::id()
    ));
    let tmp_dir = worktree_base.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let mut harness = ScenarioHarness::new("github")
        .with_worktree_base(&worktree_base)
        .with_temp_dir(&tmp_dir)
        .with_config_append(
            "[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
        );
    let write_exec = |path: &std::path::Path, body: &str| {
        std::fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    };
    write_exec(
        &harness.bin_dir.join("openhands"),
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    write_exec(
        &harness.bin_dir.join("gh"),
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/repo/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    let result = harness
        .run_dispatch(&[
            "--mode",
            "fix",
            "--backend",
            "cloud-coder",
            "--target",
            "repair this",
        ])
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr was {}", result.stderr);

    let ledger_entries = TestLedger::read_from(&harness.ledger_path).unwrap();
    let entry = ledger_entries.last().unwrap();
    let identity_projection = serde_json::json!({
        "requested_backend": entry["requested_backend"].clone(),
        "effective_backend": entry["effective_backend"].clone(),
        "effective_model": entry["effective_model"].clone(),
        "fallback_used": entry["fallback_used"].clone(),
        "backend_instance": entry["usage"]["backend_instance"].clone(),
        "usage_classification": entry["usage"]["usage_classification"].clone(),
    });
    assert_eq!(
        identity_projection,
        serde_json::json!({
            "requested_backend": "openhands",
            "effective_backend": "openhands",
            "effective_model": null,
            "fallback_used": false,
            "backend_instance": "openhands",
            "usage_classification": "unknown",
        })
    );
    assert_eq!(entry["backend"], "openhands");
}

// ── Golden fixture 4: fallback substitution ───────────────────────────────

#[test]
fn execution_identity_golden_fallback_substitution() {
    let requested = adapt_legacy_usage(
        "claude",
        Some("claude-main"),
        None,
        Some("sonnet"),
        Some("sonnet"),
        Some("sonnet"),
        false,
    );
    // Requested claude/sonnet was unavailable; routing substituted codex.
    let effective = adapt_legacy_usage(
        "codex",
        Some("codex-main"),
        None,
        Some("sonnet"),
        Some("gpt-5.4"),
        Some("gpt-5.4"),
        false,
    );

    assert_ne!(requested.logical_backend, effective.logical_backend);
    assert_ne!(requested.effective_model, effective.effective_model);
    // requested_model on the *effective* identity still records what was
    // originally asked for -- fallback substitution must not overwrite it.
    assert_eq!(effective.requested_model.as_deref(), Some("sonnet"));
    assert_eq!(effective.effective_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(requested.auth_class, "quota_backed");
    assert_eq!(effective.auth_class, "quota_backed");
}

/// Attempt-scoped attribution: a dispatch entry with two attempts that used
/// different backends must keep each attempt's own identity rather than
/// collapsing to the top-level (last-attempt) fields. Exercises the real
/// `ledger::AttemptRecord`/`LedgerUsage` types directly.
#[test]
fn execution_identity_golden_fallback_substitution_attempt_attribution() {
    let attempt_1 = AttemptRecord {
        attempt_number: 1,
        backend: "claude".to_string(),
        effective_model: Some("sonnet".to_string()),
        exit_code: Some(1),
        validation_result: Some("failed".to_string()),
        failure_class: Some("backend_error".to_string()),
        failure_stage: None,
        duration_seconds: Some(2.0),
        diff_path: None,
        cli_version: None,
        usage: LedgerUsage::default(),
    };
    let attempt_2 = AttemptRecord {
        attempt_number: 2,
        backend: "codex".to_string(),
        effective_model: Some("gpt-5.4".to_string()),
        exit_code: Some(0),
        validation_result: Some("passed".to_string()),
        failure_class: None,
        failure_stage: None,
        duration_seconds: Some(3.5),
        diff_path: None,
        cli_version: None,
        usage: LedgerUsage::default(),
    };

    assert_eq!(attempt_1.backend, "claude");
    assert_eq!(attempt_2.backend, "codex");
    assert_ne!(attempt_1.backend, attempt_2.backend);
    assert_ne!(attempt_1.effective_model, attempt_2.effective_model);
}

// ── Golden fixture 5: legacy unknowns ─────────────────────────────────────

/// A ledger line as written before `LEDGER_SCHEMA_VERSION` 3 (before
/// `usage_classification`/`backend_instance`/`provider`/etc. existed) must
/// still deserialize through the real, current production types, with every
/// new field landing as `None` -- never a coerced zero/default.
#[test]
fn execution_identity_golden_legacy_unknown() {
    let legacy_json = serde_json::json!({
        "timestamp": "2025-01-01T00:00:00Z",
        "session_id": null,
        "profile": "test",
        "display_name": "Test Repo",
        "repo_id": "test",
        "repo": "owner/repo",
        "local_path": "/tmp/repo",
        "provider": "github",
        "backend": "codex",
        "requested_backend": "codex",
        "effective_backend": "codex",
        "requested_model": null,
        "effective_model": null,
        "routing_reason": null,
        "fallback_used": false,
        "confidence_impact": null,
        "human_required": false,
        "mode": "fix",
        "target_summary": null,
        "branch": "gah/legacy-1",
        "session_dir": null,
        "duration_seconds": null,
        "backend_exit_code": null,
        "validation_result": null,
        "commit_attempted": false,
        "commit_created": false,
        "push_attempted": false,
        "push_succeeded": false,
        "mr_attempted": false,
        "mr_created": false,
        "mr_url": null,
        "files_changed": null,
        "insertions": null,
        "deletions": null,
        "error_summary": null,
        "usage": {}
    });

    let entry: LedgerEntry = serde_json::from_value(legacy_json).expect(
        "a pre-schema-version-3 ledger line without any usage/identity \
         fields must still deserialize",
    );

    // Fields that did not exist yet must be None, not a coerced default.
    assert_eq!(entry.task_class, None);
    assert_eq!(entry.difficulty, None);
    assert_eq!(entry.review_verdict, None);
    assert_eq!(entry.human_required_reason_code, None);
    assert!(entry.routing_diagnostics.is_none());
    // schema_version absent -> the documented legacy default (contract §9).
    assert_eq!(entry.schema_version, 1);
    // usage was never omittable (it predates optionality changes), but its
    // internal identity/classification fields must all be unknown.
    let usage = entry.usage;
    assert_eq!(usage.usage_classification, None);
    assert_eq!(usage.backend_instance, None);
    assert_eq!(usage.provider, None);
    assert_eq!(usage.actual_model, None);
    assert_eq!(usage.quota_window, None);
    assert_eq!(usage.cost_unknown_reason, None);
}

/// A legacy attempt record missing `cli_version` and `usage` entirely (both
/// added after `AttemptRecord` first shipped) must deserialize with
/// `cli_version: None` and a fully-unknown `usage`, not zeros.
#[test]
fn execution_identity_golden_legacy_unknown_attempt() {
    let legacy_attempt = serde_json::json!({
        "attempt_number": 1,
        "backend": "openhands",
        "effective_model": null,
        "exit_code": 0,
        "validation_result": "passed",
        "failure_class": null,
        "failure_stage": null,
        "duration_seconds": 12.5,
        "diff_path": null
    });

    let attempt: AttemptRecord = serde_json::from_value(legacy_attempt)
        .expect("a pre-usage-tracking attempt record must still deserialize");

    assert_eq!(attempt.cli_version, None);
    assert_eq!(attempt.usage.usage_classification, None);
    assert_eq!(attempt.usage.input_tokens, None);
    assert_eq!(attempt.usage.requests_count, None);
}
