#![cfg(test)]

//! Shared test-only helpers. `PATH_LOCK` is process-wide because `PathGuard`
//! mutates the global `PATH` env var; every module that touches PATH in tests
//! must go through this single lock, or two independently-locked mutations
//! (one per module) can interleave and corrupt PATH for the rest of the run.
//!
//! `EXEC_LOCK` is process-wide for a different reason: `cargo test` runs
//! tests in parallel threads within one process, and `Command::spawn`'s
//! underlying `fork()` duplicates the whole process's file descriptor table
//! into the child -- including any other thread's momentarily-open fd to a
//! freshly-written temp binary. If that fork happens while this thread is
//! between writing/chmod'ing and execing its own temp binary, the kernel can
//! return ETXTBSY. Every test that writes a temp binary and then spawns it
//! must hold `ExecGuard` for its entire body (not just the write+chmod), so
//! no other thread's fork() can ever land inside that window.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static PATH_LOCK: Mutex<()> = Mutex::new(());
static EXEC_LOCK: Mutex<()> = Mutex::new(());
pub(crate) static LEDGER_LOCK: Mutex<()> = Mutex::new(());

pub struct LedgerEnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl LedgerEnvGuard {
    /// Sets `GAH_LEDGER_PATH` to `path` for the duration of the guard.
    /// Process-wide lock because `GAH_LEDGER_PATH` is a global env var —
    /// without this lock, parallel tests that derive the ledger path from
    /// `artifact_root` silently read/write the wrong file when a status
    /// test momentarily sets the env var.
    pub fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = LEDGER_LOCK.lock().unwrap();
        std::env::set_var("GAH_LEDGER_PATH", path);
        Self { _lock: lock }
    }
}

impl Drop for LedgerEnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("GAH_LEDGER_PATH");
    }
}

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

impl Default for ExecGuard {
    fn default() -> Self {
        Self::new()
    }
}

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
