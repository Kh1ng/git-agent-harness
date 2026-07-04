#![cfg(test)]

//! Shared test-only helpers. `PATH_LOCK` is process-wide because `PathGuard`
//! mutates the global `PATH` env var; every module that touches PATH in tests
//! must go through this single lock, or two independently-locked mutations
//! (one per module) can interleave and corrupt PATH for the rest of the run.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static PATH_LOCK: Mutex<()> = Mutex::new(());

pub struct PathGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl PathGuard {
    pub fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = PATH_LOCK.lock().unwrap();
        let original = std::env::var_os("PATH");
        let requested = path.as_ref();
        let combined = match (requested.is_empty(), &original) {
            (true, Some(existing)) => existing.clone(),
            (true, None) => OsString::new(),
            (false, Some(existing)) => {
                let mut joined = OsString::from(requested);
                joined.push(":");
                joined.push(existing);
                joined
            }
            (false, None) => OsString::from(requested),
        };
        std::env::set_var("PATH", combined);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}
