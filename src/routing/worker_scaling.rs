//! Effective worker limits: the profile's baseline plus `WorkerScaling`.
use crate::{
    config::{Defaults, GahConfig, Profile},
    execution_identity::ExecutionIdentity,
    quota_store::QuotaObservationRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerLimits {
    /// `max_parallel_workers` before any scaling.
    pub baseline_workers: u32,
    pub workers: u32,
    /// Extra concurrent runs granted per `max_concurrent_per_model` key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_per_model: BTreeMap<String, u32>,
    /// One operator-readable line per grant or refusal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Compute the limits for `profile` from account quota `records`.
pub fn worker_limits(
    defaults: &Defaults,
    profile: &Profile,
    records: &[QuotaObservationRecord],
    now: OffsetDateTime,
) -> WorkerLimits {
    let baseline = profile.max_parallel_workers();
    let scaling = &profile.worker_scaling;
    let mut limits = WorkerLimits {
        baseline_workers: baseline,
        workers: baseline,
        ..Default::default()
    };

    if scaling.enabled && scaling.extra_per_model > 0 {
        let ceiling = scaling
            .max_workers
            .unwrap_or(baseline.saturating_mul(2))
            .max(baseline);
        for (key, identity) in capped_models(defaults, profile) {
            let room = ceiling - limits.workers;
            if room == 0 {
                limits.notes.push(format!(
                    "automatic scaling is at its {ceiling} worker ceiling"
                ));
                break;
            }
            let remaining =
                super::subscription::min_fresh_remaining_percent(&identity, records, now);
            match remaining {
                Some(remaining) if remaining >= scaling.min_remaining_percent => {
                    let extra = scaling.extra_per_model.min(room);
                    limits.workers += extra;
                    limits.notes.push(format!(
                        "{key}: +{extra} ({remaining:.0}% left in its tightest quota window)"
                    ));
                    limits.extra_per_model.insert(key, extra);
                }
                Some(remaining) => limits.notes.push(format!(
                    "{key}: not scaled ({remaining:.0}% left, needs {:.0}%)",
                    scaling.min_remaining_percent
                )),
                None => limits
                    .notes
                    .push(format!("{key}: not scaled (no fresh quota reading)")),
            }
        }
    }

    if scaling.boost_workers > 0 {
        match boost_is_active(scaling.boost_until.as_deref(), now) {
            Ok(true) => {
                let boost = scaling.boost_workers;
                limits.workers = limits.workers.saturating_add(boost);
                let targets: Vec<&String> = match &scaling.boost_model {
                    Some(model) => vec![model],
                    None => profile.max_concurrent_per_model.keys().collect(),
                };
                // A model without a cap is already unlimited.
                for key in targets
                    .into_iter()
                    .filter(|key| profile.max_concurrent_per_model.contains_key(*key))
                {
                    *limits.extra_per_model.entry(key.clone()).or_default() += boost;
                }
                limits.notes.push(format!(
                    "manual boost: +{boost} for {}{}",
                    scaling
                        .boost_model
                        .as_deref()
                        .unwrap_or("every capped model"),
                    scaling
                        .boost_until
                        .as_deref()
                        .map(|until| format!(" until {until}"))
                        .unwrap_or_default()
                ));
            }
            Ok(false) => limits.notes.push("manual boost has expired".into()),
            Err(until) => limits.notes.push(format!(
                "manual boost ignored: boost_until '{until}' is not an RFC 3339 time"
            )),
        }
    }
    limits
}

/// [`worker_limits`] against the node's quota store, which is read only when
/// automatic scaling needs it.
pub fn current_worker_limits(
    defaults: &Defaults,
    profile: &Profile,
    now: OffsetDateTime,
) -> WorkerLimits {
    let records = if profile.worker_scaling.enabled {
        crate::quota_store::load_account_observations()
    } else {
        Vec::new()
    };
    worker_limits(defaults, profile, &records, now)
}

fn boost_is_active(until: Option<&str>, now: OffsetDateTime) -> Result<bool, String> {
    match until {
        None => Ok(true),
        Some(until) => OffsetDateTime::parse(until, &Rfc3339)
            .map(|until| until > now)
            .map_err(|_| until.to_string()),
    }
}

/// Every routing candidate that has a `max_concurrent_per_model` cap, highest
/// priority first, so the preferred model is scaled before the ceiling binds.
fn capped_models(defaults: &Defaults, profile: &Profile) -> Vec<(String, ExecutionIdentity)> {
    let routing = profile.effective_routing(defaults);
    let mut candidates: Vec<_> = [
        &routing.improve_candidates,
        &routing.review_candidates,
        &routing.pm_candidates,
    ]
    .into_iter()
    .flatten()
    .flatten()
    .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.priority));

    let mut models = Vec::<(String, ExecutionIdentity)>::new();
    for candidate in candidates {
        let identity = routing.execution_identity_for_candidate(candidate);
        let key = cap_key(&identity);
        if profile.max_concurrent_per_model.contains_key(&key)
            && !models.iter().any(|(seen, _)| *seen == key)
        {
            models.push((key, identity));
        }
    }
    models
}

/// The `max_concurrent_per_model` key for one execution identity.
pub fn cap_key(identity: &ExecutionIdentity) -> String {
    format!(
        "{}/{}",
        identity.logical_backend,
        identity.effective_model.as_deref().unwrap_or("")
    )
}

/// Replace the profile's baseline limits with its scaled ones for this
/// process. `worker_scaling` is reset afterwards, so applying twice, or
/// computing limits from the applied profile, cannot scale a second time.
pub fn apply(cfg: &mut GahConfig, profile_name: &str) -> Option<WorkerLimits> {
    let limits = current_worker_limits(
        &cfg.defaults,
        cfg.profiles.get(profile_name)?,
        OffsetDateTime::now_utc(),
    );
    let profile = cfg.profiles.get_mut(profile_name)?;
    profile.max_parallel_workers = Some(limits.workers);
    for (key, extra) in &limits.extra_per_model {
        if let Some(cap) = profile.max_concurrent_per_model.get_mut(key) {
            *cap = cap.saturating_add(*extra);
        }
    }
    profile.worker_scaling = Default::default();
    Some(limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CandidateConfig;
    use std::collections::HashMap;

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-10-05T06:00:00Z", &Rfc3339).unwrap()
    }

    fn profile() -> Profile {
        let mut profile = crate::routing::test_support::profile();
        profile.max_parallel_workers = Some(3);
        profile.max_concurrent_per_model =
            HashMap::from([("codex/gpt".into(), 2), ("claude/sonnet".into(), 1)]);
        profile.routing.improve_candidates = Some(vec![
            candidate("claude", "sonnet", 110),
            candidate("codex", "gpt", 120),
        ]);
        profile
    }

    fn candidate(backend: &str, model: &str, priority: i32) -> CandidateConfig {
        CandidateConfig {
            backend: backend.into(),
            model: Some(model.into()),
            priority,
            ..Default::default()
        }
    }

    fn reading(backend: &str, window: &str, remaining: f64) -> QuotaObservationRecord {
        serde_json::from_value(serde_json::json!({
            "backend": backend, "quota_window": window,
            "quota_remaining_percent": remaining,
            "quota_reset_at": "2026-10-05T09:00:00Z",
            "observed_at": "2026-10-05T05:55:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn disabled_scaling_keeps_the_baseline() {
        let limits = worker_limits(
            &Defaults::default(),
            &profile(),
            &[reading("codex", "5-hour", 100.0)],
            now(),
        );
        assert_eq!((limits.baseline_workers, limits.workers), (3, 3));
        assert!(limits.extra_per_model.is_empty());
    }

    #[test]
    fn a_model_scales_only_while_every_fresh_window_has_headroom() {
        let mut profile = profile();
        profile.worker_scaling.enabled = true;
        let records = [
            reading("codex", "5-hour", 80.0),
            reading("codex", "weekly", 60.0),
            reading("claude", "5-hour", 90.0),
            reading("claude", "weekly", 20.0),
        ];
        let limits = worker_limits(&Defaults::default(), &profile, &records, now());
        assert_eq!(limits.workers, 4);
        assert_eq!(
            limits.extra_per_model,
            BTreeMap::from([("codex/gpt".to_string(), 1)])
        );
    }

    #[test]
    fn unknown_or_stale_quota_never_scales() {
        let mut profile = profile();
        profile.worker_scaling.enabled = true;
        let mut stale = reading("codex", "5-hour", 100.0);
        stale.observed_at = Some("2026-10-05T04:00:00Z".into());
        let limits = worker_limits(&Defaults::default(), &profile, &[stale], now());
        assert_eq!(limits.workers, 3);
    }

    #[test]
    fn the_ceiling_goes_to_the_highest_priority_model_first() {
        let mut profile = profile();
        profile.worker_scaling.enabled = true;
        profile.worker_scaling.extra_per_model = 2;
        profile.worker_scaling.max_workers = Some(5);
        let records = [
            reading("codex", "5-hour", 100.0),
            reading("claude", "5-hour", 100.0),
        ];
        let limits = worker_limits(&Defaults::default(), &profile, &records, now());
        assert_eq!(limits.workers, 5);
        assert_eq!(
            limits.extra_per_model,
            BTreeMap::from([("codex/gpt".to_string(), 2)])
        );
    }

    #[test]
    fn a_boost_adds_workers_until_it_expires_and_ignores_the_ceiling() {
        let mut profile = profile();
        profile.worker_scaling.max_workers = Some(3);
        profile.worker_scaling.boost_workers = 2;
        profile.worker_scaling.boost_model = Some("codex/gpt".into());
        profile.worker_scaling.boost_until = Some("2026-10-05T08:00:00Z".into());
        let limits = worker_limits(&Defaults::default(), &profile, &[], now());
        assert_eq!(limits.workers, 5);
        assert_eq!(limits.extra_per_model["codex/gpt"], 2);

        profile.worker_scaling.boost_until = Some("2026-10-05T05:00:00Z".into());
        assert_eq!(
            worker_limits(&Defaults::default(), &profile, &[], now()).workers,
            3
        );
        profile.worker_scaling.boost_until = Some("soon".into());
        assert_eq!(
            worker_limits(&Defaults::default(), &profile, &[], now()).workers,
            3
        );
    }

    #[test]
    fn a_boost_without_a_model_raises_every_cap() {
        let mut profile = profile();
        profile.worker_scaling.boost_workers = 1;
        let limits = worker_limits(&Defaults::default(), &profile, &[], now());
        assert_eq!(limits.workers, 4);
        assert_eq!(limits.extra_per_model.len(), 2);
    }

    #[test]
    fn apply_rewrites_the_profile_once() {
        let mut profile = profile();
        profile.worker_scaling.boost_workers = 2;
        profile.worker_scaling.boost_model = Some("codex/gpt".into());
        let mut cfg = GahConfig {
            defaults: Defaults::default(),
            profiles: HashMap::from([("p".to_string(), profile)]),
            context: Default::default(),
        };
        apply(&mut cfg, "p").unwrap();
        let second = apply(&mut cfg, "p").unwrap();
        let applied = &cfg.profiles["p"];
        assert_eq!(applied.max_parallel_workers, Some(5));
        assert_eq!(applied.max_concurrent_per_model["codex/gpt"], 4);
        assert_eq!(second.baseline_workers, 5);
    }
}
