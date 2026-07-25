use anyhow::Result;
use std::hash::{Hash, Hasher};

/// Hash only non-secret effective configuration plus durable availability.
/// `list_scopes` evaluates expiry against `now`, so a cooldown becoming
/// eligible changes this fingerprint even when the state file is untouched.
pub(super) fn route_state_fingerprint(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    now: time::OffsetDateTime,
) -> Result<String> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_canonical_json(&serde_json::to_value(profile)?, &mut hasher);
    hash_canonical_json(
        &serde_json::to_value(profile.effective_routing(&cfg.defaults))?,
        &mut hasher,
    );
    format!(
        "{:?}",
        crate::availability::list_scopes(&crate::availability::resolve_state_path(), now)?
    )
    .hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

pub(super) fn record_capacity_deferral(
    cfg: &crate::config::GahConfig,
    args: &crate::dispatch::DispatchArgs,
    label: &str,
    work_id: Option<&str>,
    error: &anyhow::Error,
) -> Result<Option<String>> {
    let route_state = route_state_fingerprint(cfg, &args.profile, time::OffsetDateTime::now_utc())
        .ok()
        .map(|fingerprint| format!(" route_state={fingerprint}"))
        .unwrap_or_default();
    crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::DispatchFinished,
        Some(args.profile.as_str()),
        work_id,
        args.run_id.as_deref(),
        format!("{label}: deferred_capacity: {error:#}{route_state}"),
    )?;
    Ok(Some(capacity_deferral_outcome(label, error)))
}

fn capacity_deferral_outcome(label: &str, error: &anyhow::Error) -> String {
    let capacity = if crate::dispatch::node_capacity_deferred_error(error) {
        "node"
    } else {
        "configured route"
    };
    if let Some(attempts) = crate::dispatch::post_attempt_capacity_deferral(error) {
        return format!(
            "Deferred {label} fallback because {capacity} capacity is busy after {attempts} backend attempt(s); prior backend outcome preserved"
        );
    }
    format!("Deferred {label} because {capacity} capacity is busy; no backend launched")
}

/// Hash JSON objects by sorted key, not serializer iteration order. Profile
/// configuration contains HashMaps whose order is intentionally randomized
/// each time the recurring loop reloads TOML; hashing `serde_json::to_string`
/// directly made an unchanged route state look different on every tick.
fn hash_canonical_json(value: &serde_json::Value, hasher: &mut impl Hasher) {
    match value {
        serde_json::Value::Null => 0_u8.hash(hasher),
        serde_json::Value::Bool(value) => {
            1_u8.hash(hasher);
            value.hash(hasher);
        }
        serde_json::Value::Number(value) => {
            2_u8.hash(hasher);
            value.to_string().hash(hasher);
        }
        serde_json::Value::String(value) => {
            3_u8.hash(hasher);
            value.hash(hasher);
        }
        serde_json::Value::Array(values) => {
            4_u8.hash(hasher);
            values.len().hash(hasher);
            for value in values {
                hash_canonical_json(value, hasher);
            }
        }
        serde_json::Value::Object(values) => {
            5_u8.hash(hasher);
            values.len().hash(hasher);
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                key.hash(hasher);
                hash_canonical_json(&values[key], hasher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::capacity_deferral_outcome;

    #[test]
    fn prelaunch_node_deferral_says_no_backend_launched() {
        let error = anyhow::Error::new(crate::controller::NodeAdmissionDeferred(
            "memory reserve".into(),
        ));
        assert_eq!(
            capacity_deferral_outcome("review_mr", &error),
            "Deferred review_mr because node capacity is busy; no backend launched"
        );
    }

    #[test]
    fn review_fallback_deferral_preserves_prior_attempt_attribution() {
        let error = crate::dispatch::contextualize_capacity_deferral(
            anyhow::Error::new(crate::controller::NodeAdmissionDeferred(
                "memory reserve".into(),
            )),
            1,
        );
        let outcome = capacity_deferral_outcome("review_mr", &error);
        assert_eq!(
            outcome,
            "Deferred review_mr fallback because node capacity is busy after 1 backend attempt(s); prior backend outcome preserved"
        );
        assert!(!outcome.contains("no backend launched"));
    }
}
