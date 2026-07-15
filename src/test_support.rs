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
static AVAILABILITY_LOCK: Mutex<()> = Mutex::new(());
static CLAIM_STATE_LOCK: Mutex<()> = Mutex::new(());

/// Scoped override for the process-global availability store path used by
/// tests. Just like the ledger override, it must be restored before another
/// parallel test can observe process state.
pub struct AvailabilityEnvGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

pub struct ClaimStateEnvGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl ClaimStateEnvGuard {
    pub fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = CLAIM_STATE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let original = std::env::var_os("GAH_CLAIM_STATE_PATH");
        std::env::set_var("GAH_CLAIM_STATE_PATH", path);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for ClaimStateEnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => std::env::set_var("GAH_CLAIM_STATE_PATH", path),
            None => std::env::remove_var("GAH_CLAIM_STATE_PATH"),
        }
    }
}

impl AvailabilityEnvGuard {
    pub fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = AVAILABILITY_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let original = std::env::var_os("GAH_AVAILABILITY_PATH");
        std::env::set_var("GAH_AVAILABILITY_PATH", path);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for AvailabilityEnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => std::env::set_var("GAH_AVAILABILITY_PATH", path),
            None => std::env::remove_var("GAH_AVAILABILITY_PATH"),
        }
    }
}

pub struct ExecGuard {
    _lock: MutexGuard<'static, ()>,
}

impl ExecGuard {
    pub fn new() -> Self {
        Self {
            // A failed test must not poison the shared execution lock and hide
            // the real failure behind dozens of unrelated test failures.
            _lock: EXEC_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
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
        let lock = PATH_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
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
