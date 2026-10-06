import re

with open("src/quota_store.rs", "r") as f:
    content = f.read()

helper = """
fn normalize_quota_percent(val: &mut serde_json::Value) {
    if let Some(obj) = val.as_object_mut() {
        if obj.get("quota_remaining_percent").is_none() {
            if let Some(used) = obj.get("quota_used_percent").and_then(|v| v.as_f64()) {
                obj.insert("quota_remaining_percent".to_string(), serde_json::json!(100.0 - used));
            }
        }
        obj.remove("quota_used_percent");
    }
}
"""

content = content.replace("pub fn load(", helper + "\npub fn load(")

# Update load loop
old_load_loop = """        match serde_json::from_str::<QuotaObservationRecord>(line) {
            Ok(mut rec) => {
                rec.backend = crate::config::canonical_backend_name(&rec.backend).to_string();
                records.push(rec);
            }
            Err(_) => continue,
        }"""
new_load_loop = """        let mut val = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(val) => val,
            Err(_) => continue,
        };
        normalize_quota_percent(&mut val);
        match serde_json::from_value::<QuotaObservationRecord>(val) {
            Ok(mut rec) => {
                rec.backend = crate::config::canonical_backend_name(&rec.backend).to_string();
                records.push(rec);
            }
            Err(_) => continue,
        }"""

content = content.replace(old_load_loop, new_load_loop)

# Update parse_external_observation
old_parse = """    let mut record: QuotaObservationRecord = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("invalid quota observation schema"))?;"""
new_parse = """    let mut val = value;
    normalize_quota_percent(&mut val);
    let mut record: QuotaObservationRecord = serde_json::from_value(val)
        .map_err(|_| anyhow::anyhow!("invalid quota observation schema"))?;"""

content = content.replace(old_parse, new_parse)

with open("src/quota_store.rs", "w") as f:
    f.write(content)
