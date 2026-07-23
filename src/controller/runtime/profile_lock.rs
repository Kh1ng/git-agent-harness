use anyhow::Result;
use fs2::FileExt;
use std::fs::OpenOptions;
use std::path::PathBuf;

/// The lock is scoped by profile name AND config file identity: a profile
/// is really a named entry *within a specific config file*, so two
/// different config files that happen to define a same-named profile (e.g.
/// separate test fixtures, or a user's dev vs. prod config) are genuinely
/// independent and must not block each other. Two invocations against the
/// same config file (the real-world incident this guards against: the
/// daemon and an ad-hoc `--once` both using the default
/// `~/.config/gah/config.toml`) hash to the same lock file. The lock must
/// not live under `XDG_STATE_HOME`: backend wrappers and service managers may
/// use different XDG environments while still operating the same profile.
pub(crate) fn loop_lock_path(profile_name: &str, config_path: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let canonical_config =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical_config.hash(&mut hasher);
    let lock_dir = canonical_config
        .parent()
        .map(|parent| parent.join(".gah-locks"))
        .unwrap_or_else(|| PathBuf::from(".gah-locks"));
    lock_dir.join(format!(
        "loop-{}-{:x}.lock",
        profile_name.replace('/', "_"),
        hasher.finish()
    ))
}

/// Held for the lifetime of a single gah invocation (daemon loop, `--once`,
/// or a manual `dispatch`) that performs real execution -- spawning
/// backends, claiming tickets, writing ledger entries -- for a profile.
/// Dropping it releases the underlying flock.
// The File is never read again -- it exists only so its flock is released on
// Drop, when the guard goes out of scope at the end of the invocation.
pub struct ProfileLock {
    pub(crate) _file: std::fs::File,
}

/// Acquire the exclusive per-profile execution lock so that only one gah
/// process at a time can do real execution work for a given profile of a
/// given config file.
///
/// Callers (see `main.rs`) must call this exactly ONCE per process, at the
/// outermost entry point for whichever command they're running, and hold
/// the returned guard for the rest of that invocation. Do not call this
/// again from within an already-locked process (e.g. from inside
/// `run_loop`'s per-iteration `run_once` calls) -- POSIX flock exclusivity
/// is per open-file-description, not per-process, so a second `open()` +
/// `try_lock_exclusive()` from the same process would conflict with its own
/// already-held lock and deadlock.
pub fn acquire_profile_lock(
    profile_name: &str,
    config_path: &std::path::Path,
) -> Result<ProfileLock> {
    let lock_path = loop_lock_path(profile_name, config_path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "gah already running for profile '{profile_name}' (lock: {})",
            lock_path.display()
        )
    })?;
    Ok(ProfileLock { _file: lock })
}

/// Reload the config from disk for `run_loop`'s per-iteration hot-reload,
/// validating that `profile_name` is still resolvable in the freshly loaded
/// config. A parse-clean reload that dropped or renamed this exact profile
/// (e.g. an operator edit mid-run) is just as unsafe to dispatch against as a
/// read failure -- callers must treat both errors identically (fall back to
/// the last-known-good config) rather than adopting a config the running
/// profile no longer resolves against.
pub(crate) fn reload_config_for_profile(
    config_path: &std::path::Path,
    profile_name: &str,
) -> Result<crate::config::GahConfig> {
    let loaded = crate::config::load(config_path.to_str())?;
    crate::config::get_profile(&loaded, profile_name)?;
    Ok(loaded)
}
