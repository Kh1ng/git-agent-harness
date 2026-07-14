use std::time::{SystemTime, UNIX_EPOCH};

pub(in crate::dispatch) fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{seconds}-{}", uuid::Uuid::new_v4().simple())
}
