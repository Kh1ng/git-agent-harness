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
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

static PATH_LOCK: Mutex<()> = Mutex::new(());
static EXEC_LOCK: Mutex<()> = Mutex::new(());
static AVAILABILITY_LOCK: Mutex<()> = Mutex::new(());
static QUOTA_STORE_LOCK: Mutex<()> = Mutex::new(());
static CLAIM_STATE_LOCK: Mutex<()> = Mutex::new(());
static MISTRAL_ADMIN_KEY_LOCK: Mutex<()> = Mutex::new(());

// A process-global env override would race other tests calling config::load().
// The explicit test fixture is thread-local; every other test sees an empty
// private directory, even if the runner has GAH_CANONICAL_CONFIG set.
static CANONICAL_CONFIG_TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
thread_local! {
    static CANONICAL_CONFIG_TEST_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn canonical_config_path() -> PathBuf {
    CANONICAL_CONFIG_TEST_OVERRIDE
        .with(|cell| cell.borrow().clone())
        .unwrap_or_else(|| {
            CANONICAL_CONFIG_TEST_DIR
                .get_or_init(|| tempfile::tempdir().expect("create isolated canonical config dir"))
                .path()
                .join("canonical.toml")
        })
}

pub(crate) fn set_canonical_config_override(path: impl Into<PathBuf>) {
    CANONICAL_CONFIG_TEST_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(path.into()));
}

pub(crate) fn clear_canonical_config_override() {
    CANONICAL_CONFIG_TEST_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
}

#[test]
fn unit_tests_do_not_inherit_operator_canonical_config() {
    clear_canonical_config_override();
    let path = canonical_config_path();
    assert_ne!(
        path,
        crate::config::default_config_dir().join("canonical.toml")
    );
    if let Some(operator_path) = std::env::var_os("GAH_CANONICAL_CONFIG") {
        assert_ne!(path, PathBuf::from(operator_path));
    }
    assert!(!path.exists());
}

/// Scoped override for `MISTRAL_ADMIN_API_KEY`, serialized against every
/// other test that reads/writes this process-global env var (in both
/// `usage::vibe_admin` and `quota_store`).
pub struct MistralAdminKeyEnvGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl MistralAdminKeyEnvGuard {
    pub fn set(value: &str) -> Self {
        let lock = MISTRAL_ADMIN_KEY_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let original = std::env::var_os("MISTRAL_ADMIN_API_KEY");
        std::env::set_var("MISTRAL_ADMIN_API_KEY", value);
        Self {
            _lock: lock,
            original,
        }
    }

    pub fn unset() -> Self {
        let lock = MISTRAL_ADMIN_KEY_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let original = std::env::var_os("MISTRAL_ADMIN_API_KEY");
        std::env::remove_var("MISTRAL_ADMIN_API_KEY");
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for MistralAdminKeyEnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => std::env::set_var("MISTRAL_ADMIN_API_KEY", value),
            None => std::env::remove_var("MISTRAL_ADMIN_API_KEY"),
        }
    }
}

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

pub struct QuotaStoreEnvGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl QuotaStoreEnvGuard {
    pub fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = QUOTA_STORE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let original = std::env::var_os("GAH_QUOTA_STORE_PATH");
        std::env::set_var("GAH_QUOTA_STORE_PATH", path);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for QuotaStoreEnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => std::env::set_var("GAH_QUOTA_STORE_PATH", path),
            None => std::env::remove_var("GAH_QUOTA_STORE_PATH"),
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
