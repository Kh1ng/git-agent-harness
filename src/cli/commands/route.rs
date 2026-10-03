//! Runtime routing uses the same identities, quota parser and availability store as dispatch.
use anyhow::Result;
use std::io::Read;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub fn run(
    profile_name: &str,
    model: Option<&str>,
    backend: Option<&str>,
    instance: Option<&str>,
    config_path: Option<&str>,
) -> Result<()> {
    let cfg = crate::config::load(config_path)?;
    let profile = crate::config::get_profile(&cfg, profile_name)?;
    let now = OffsetDateTime::now_utc();
    let Some(backend) = backend else {
        println!(
            "{}",
            serde_json::to_string(&crate::routing::subscription::handoff_routes(
                &cfg.defaults,
                profile,
                model,
                now
            )?)?
        );
        return Ok(());
    };
    let routing = profile.effective_routing(&cfg.defaults);
    let logical = instance
        .and_then(|name| routing.backend_instances.get(name))
        .and_then(|entry| entry.logical_backend.as_deref())
        .unwrap_or(backend);
    if let Some(instance) = instance {
        anyhow::ensure!(
            routing
                .backend_instances
                .get(instance)
                .is_some_and(|entry| entry.enabled() && entry.runner_kind == backend),
            "Invalid backend instance for {backend}"
        );
    }
    let identity = routing.execution_identity_for_candidate(&crate::config::CandidateConfig {
        backend: logical.into(),
        instance: instance.map(str::to_owned),
        model: model.map(str::to_owned),
        ..Default::default()
    });
    let mut text = String::new();
    std::io::stdin().take(64 * 1024).read_to_string(&mut text)?;
    let parsed = crate::quota_parser::parse(backend, &text, now);
    let kind = match parsed.as_ref().map(|failure| failure.kind) {
        Some(crate::quota_parser::FailureKind::QuotaExhausted) => "hard",
        Some(crate::quota_parser::FailureKind::RateLimited) => "transient",
        _ => "other",
    };
    let reset = parsed
        .as_ref()
        .and_then(|failure| failure.reset_at.as_deref())
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
    if kind == "hard" {
        crate::availability::record_unavailable_for_identity(
            &crate::availability::resolve_state_path(),
            &identity,
            crate::availability::Reason::QuotaExhausted,
            crate::availability::Source::BackendError,
            reset,
            Some("Chat subscription quota exhausted".into()),
            now,
        )?;
    }
    println!(
        "{}",
        serde_json::json!({ "kind": kind, "resetAt": reset.map(|value| value.unix_timestamp() * 1000), "retryAfterMs": parsed.and_then(|failure| failure.retry_after_seconds).map(|seconds| seconds * 1000) })
    );
    Ok(())
}
