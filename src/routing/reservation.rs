use super::types::SkippedBackend;
use anyhow::Result;
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

fn concurrency_key(backend: &str, model: Option<&str>) -> String {
    format!("{backend}/{}", model.unwrap_or(""))
}

fn concurrency_counters() -> &'static Mutex<HashMap<String, u32>> {
    static COUNTERS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Current number of in-flight dispatches this process has running against a
/// given backend+model pair. Backed by the same counter `ConcurrencyGuard`
/// increments/decrements. Process-wide, not persisted, not cross-process --
/// see `Profile::max_concurrent_per_model` for why that's sufficient.
pub(super) fn current_concurrent(backend: &str, model: Option<&str>) -> u32 {
    let key = concurrency_key(backend, model);
    *concurrency_counters()
        .lock()
        .unwrap()
        .get(&key)
        .unwrap_or(&0)
}

/// RAII marker for one in-flight dispatch against a backend+model pair.
/// Acquire right before the backend call starts; drop releases it -- on
/// success, error, or panic unwind, since it's a plain `Drop` impl rather
/// than a manually-called release on specific exit paths (mirrors the intent
/// of `work_claim::release_work`'s success/error coverage, just via RAII).
pub struct ConcurrencyGuard {
    key: String,
    shared_file: Option<File>,
}

impl ConcurrencyGuard {
    pub fn acquire(backend: &str, model: Option<&str>) -> Self {
        let key = concurrency_key(backend, model);
        *concurrency_counters()
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert(0) += 1;
        ConcurrencyGuard {
            key,
            shared_file: None,
        }
    }

    /// Reserve one configured backend/model slot across processes. A flock is
    /// released by the kernel if the worker dies, so a crashed actor cannot
    /// permanently consume quota capacity. This intentionally uses the
    /// smallest safe primitive: one lock file per slot, with a bounded slot
    /// count read from profile policy.
    pub fn acquire_shared(backend: &str, model: Option<&str>, cap: Option<u32>) -> Result<Self> {
        let Some(cap) = cap else {
            return Ok(Self::acquire(backend, model));
        };
        let cap = cap.max(1);
        let key = concurrency_key(backend, model);
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("gah")
            .join("concurrency");
        std::fs::create_dir_all(&root)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        key.hash(&mut hasher);
        let stem = format!("{:x}", hasher.finish());

        loop {
            for slot in 0..cap {
                let path = root.join(format!("{stem}-{slot}.lock"));
                let file = OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .truncate(false)
                    .open(path)?;
                match file.try_lock_exclusive() {
                    Ok(()) => {
                        let mut guard = Self::acquire(backend, model);
                        guard.shared_file = Some(file);
                        return Ok(guard);
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                    Err(err) => return Err(err.into()),
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        if let Some(file) = self.shared_file.take() {
            let _ = file.unlock();
        }
        let mut counters = concurrency_counters().lock().unwrap();
        if let Some(count) = counters.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counters.remove(&self.key);
            }
        }
    }
}

/// Skip a candidate already at its configured `max_concurrent_per_model`
/// cap. `None` when the pair has no configured cap (unlimited, the default)
/// or is still under it.
pub(super) fn max_concurrent_skip(
    max_concurrent: &HashMap<String, u32>,
    backend: &str,
    model: Option<&str>,
) -> Option<SkippedBackend> {
    let cap = *max_concurrent.get(&concurrency_key(backend, model))?;
    if super::current_concurrent(backend, model) >= cap {
        Some(SkippedBackend {
            backend: backend.to_string(),
            model: model.map(str::to_string),
            reason: "max_concurrent_reached".into(),
            unavailable_until: None,
        })
    } else {
        None
    }
}

#[cfg(test)]
#[path = "reservation/tests.rs"]
mod tests;
