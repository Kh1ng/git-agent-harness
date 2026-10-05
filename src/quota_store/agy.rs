//! Persist native Antigravity readings under their independent model pools.
use super::{append, QuotaObservationRecord};
use anyhow::Result;
use std::path::Path;

pub fn refresh_and_store(
    executable: &str,
    backend: &str,
    home: Option<&Path>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let records = crate::usage::agy::refresh(executable, backend, home)?;
    for record in &records {
        append(state_path, record)?;
    }
    Ok(records.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    #[test]
    fn pool_readings_throttle_the_account_probe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::now_utc();
        for backend in ["agy", "agy-second"] {
            let record: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
                "backend": backend,
                "backend_instance": format!("{backend}:google-native"),
                "quota_pool": format!("{backend}:google-native"),
                "quota_window": "weekly",
                "quota_remaining_percent": 80.0,
                "checked_at": now.format(&Rfc3339).unwrap(),
                "observed_at": now.format(&Rfc3339).unwrap()
            }))
            .unwrap();
            append(&path, &record).unwrap();
            assert!(
                super::super::maybe_refresh_backend(&path, backend, now, || panic!("not due"))
                    .is_none()
            );
        }
    }
}
