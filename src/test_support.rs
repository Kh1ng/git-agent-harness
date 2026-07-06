#![cfg(test)]

//! Shared test-only helpers. `PATH_LOCK` is process-wide because `PathGuard`
//! mutates the global `PATH` env var; every module that touches PATH in tests
//! must go through this single lock, or two independently-locked mutations
//! (one per module) can interleave and corrupt PATH for the rest of the run.
//!
//! `EXEC_LOCK` is process-wide because creating a temp binary file and immediately
//! spawning/execing it can race with another thread's fork() inheriting an
//! open write fd to a different temp binary, causing ETXTBSY (Text file busy).
//! Every code path that writes a temp binary and then execs it must hold this
//! lock across the write->chmod->spawn window.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static PATH_LOCK: Mutex<()> = Mutex::new(());
static EXEC_LOCK: Mutex<()> = Mutex::new(());

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

/// A guard that holds the `EXEC_LOCK` to prevent ETXTBSY races when
/// creating and immediately executing temporary binaries.
pub struct ExecGuard {
    _lock: MutexGuard<'static, ()>,
}

impl ExecGuard {
    pub fn new() -> Self {
        Self {
            _lock: EXEC_LOCK.lock().unwrap(),
        }
    }
}
