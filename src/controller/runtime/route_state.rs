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
