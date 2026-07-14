//! Per-dispatch build-output isolation.
//!
//! Cargo's registry and source caches are safe to share, but a writable
//! `CARGO_TARGET_DIR` is not safe to share between concurrent worktrees of the
//! same package/version. One worktree can otherwise execute another
//! worktree's freshly-built test binary. Each dispatch therefore owns a
//! target directory for its whole session and removes it on completion.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const TARGET_ROOT: &str = "cargo-targets";
const ACTIVE_LOCK: &str = ".active.lock";
const LIFECYCLE_LOCK: &str = ".cargo-targets.lifecycle.lock";

/// Stable value used in the validation-gate cache key. The concrete target
/// path is intentionally session-specific, but the isolation policy itself is
/// stable and should not invalidate a previously-proven gate every run.
pub fn validation_environment_signature(artifact_root: &str) -> Vec<(String, String)> {
    vec![(
        "CARGO_TARGET_DIR".to_string(),
        Path::new(artifact_root)
            .join("build-cache")
            .join(TARGET_ROOT)
            .join("<isolated-session>")
            .to_string_lossy()
            .into_owned(),
    )]
}

fn session_scope(path: &Path) -> &Path {
    let nested_attempt = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("attempt-") || name.starts_with("pm-run-"));
    if nested_attempt {
        path.parent().unwrap_or(path)
    } else {
        path
    }
}

fn scope_name(scope: &Path) -> String {
    let scope = session_scope(scope);
    let mut hasher = DefaultHasher::new();
    scope.hash(&mut hasher);
    let digest = hasher.finish();
    let slug: String = scope
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dispatch")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .take(72)
        .collect();
    format!("{slug}-{digest:016x}")
}

pub fn target_root(artifact_root: &str) -> PathBuf {
    Path::new(artifact_root)
        .join("build-cache")
        .join(TARGET_ROOT)
}

pub fn target_dir(artifact_root: &str, scope: &Path) -> PathBuf {
    target_root(artifact_root).join(scope_name(scope))
}

fn acquire_lifecycle_lock(artifact_root: &Path) -> Result<File> {
    let build_cache = artifact_root.join("build-cache");
    fs::create_dir_all(&build_cache)
        .with_context(|| format!("creating build-cache root {}", build_cache.display()))?;
    let lock_path = build_cache.join(LIFECYCLE_LOCK);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "opening Cargo target lifecycle lock {}",
                lock_path.display()
            )
        })?;
    lock.lock_exclusive()
        .with_context(|| format!("locking Cargo target lifecycle {}", lock_path.display()))?;
    Ok(lock)
}

/// Owns one writable Cargo target directory for one dispatch session.
///
/// The advisory lock lets automatic pruning distinguish a live build from a
/// directory left behind by SIGKILL, a host crash, or an older binary.
pub struct ScopedCargoTarget {
    artifact_root: PathBuf,
    path: PathBuf,
    lock: File,
}

impl ScopedCargoTarget {
    pub fn acquire(artifact_root: &str, scope: &Path) -> Result<Self> {
        Self::acquire_with_hook(artifact_root, scope, || {})
    }

    fn acquire_with_hook<F>(artifact_root: &str, scope: &Path, after_lock_open: F) -> Result<Self>
    where
        F: FnOnce(),
    {
        let artifact_root_path = PathBuf::from(artifact_root);
        // Creation and owner-lock acquisition must be atomic with respect to
        // pruning. Otherwise a pruner can unlink `.active.lock` after this
        // process opens it but before it locks it, leaving a live worker
        // holding an unlinked inode while Cargo recreates an unprotected path.
        let _lifecycle = acquire_lifecycle_lock(&artifact_root_path)?;
        let path = target_dir(artifact_root, scope);
        fs::create_dir_all(&path)
            .with_context(|| format!("creating isolated Cargo target {}", path.display()))?;
        let lock_path = path.join(ACTIVE_LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening Cargo target lock {}", lock_path.display()))?;
        after_lock_open();
        lock.try_lock_exclusive().with_context(|| {
            format!(
                "isolated Cargo target is already owned by another dispatch: {}",
                path.display()
            )
        })?;
        Ok(Self {
            artifact_root: artifact_root_path,
            path,
            lock,
        })
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn environment(&self) -> Vec<(String, String)> {
        vec![(
            "CARGO_TARGET_DIR".to_string(),
            self.path.to_string_lossy().into_owned(),
        )]
    }
}

impl Drop for ScopedCargoTarget {
    fn drop(&mut self) {
        // Keep removal in the same lifecycle critical section as creation and
        // pruning. The active lock remains held while waiting; pruning uses a
        // non-blocking active-lock probe, so this cannot deadlock.
        let _lifecycle = match acquire_lifecycle_lock(&self.artifact_root) {
            Ok(lock) => lock,
            Err(error) => {
                eprintln!(
                    "warning: failed to lock Cargo target lifecycle before removing {}: {error:#}",
                    self.path.display()
                );
                return;
            }
        };
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "warning: failed to remove isolated Cargo target {}: {error}",
                    self.path.display()
                );
            }
        }
        let _ = self.lock.unlock();
    }
}

/// Remove target directories that have no live owner lock. This catches
/// directories left behind when RAII cleanup could not run.
pub fn prune_inactive(artifact_root: &str, dry_run: bool) -> Result<usize> {
    // Serialize the complete scan/remove operation with target creation. In
    // particular, never unlink a lock file that a new owner has opened but
    // has not yet locked.
    let artifact_root_path = Path::new(artifact_root);
    let _lifecycle = acquire_lifecycle_lock(artifact_root_path)?;
    let root = target_root(artifact_root);
    if !root.exists() {
        return Ok(0);
    }

    let mut removed = 0;
    for entry in fs::read_dir(&root)
        .with_context(|| format!("reading Cargo target root {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let lock_path = path.join(ACTIVE_LOCK);
        let lock = match OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("opening Cargo target lock {}", lock_path.display()))
            }
        };
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        if dry_run {
            println!("  would remove inactive Cargo target {}", path.display());
        } else {
            fs::remove_dir_all(&path)
                .with_context(|| format!("removing inactive Cargo target {}", path.display()))?;
            println!("  removed inactive Cargo target {}", path.display());
        }
        let _ = lock.unlock();
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::{prune_inactive, target_dir, ScopedCargoTarget};
    use std::fs;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn attempts_in_one_session_share_a_target_but_sessions_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let session_a = tmp.path().join("sessions/a");
        let session_b = tmp.path().join("sessions/b");
        assert_eq!(
            target_dir("/artifacts", &session_a.join("attempt-1")),
            target_dir("/artifacts", &session_a.join("attempt-2"))
        );
        assert_ne!(
            target_dir("/artifacts", &session_a),
            target_dir("/artifacts", &session_b)
        );
    }

    #[test]
    fn guard_removes_its_target_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path;
        {
            let guard = ScopedCargoTarget::acquire(
                tmp.path().to_str().unwrap(),
                &tmp.path().join("session"),
            )
            .unwrap();
            path = guard.path().to_path_buf();
            fs::write(path.join("artifact"), "data").unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }

    #[test]
    fn prune_keeps_live_target_and_removes_abandoned_target() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_root = tmp.path().to_str().unwrap();
        let live = ScopedCargoTarget::acquire(artifact_root, &tmp.path().join("live")).unwrap();
        let abandoned = target_dir(artifact_root, &tmp.path().join("abandoned"));
        fs::create_dir_all(&abandoned).unwrap();
        fs::write(abandoned.join("artifact"), "data").unwrap();

        assert_eq!(prune_inactive(artifact_root, false).unwrap(), 1);
        assert!(live.path().exists());
        assert!(!abandoned.exists());
    }

    #[test]
    fn prune_cannot_unlink_a_target_between_opening_and_locking() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_root = tmp.path().join("artifacts");
        let scope = tmp.path().join("session");
        let artifact_root_text = artifact_root.to_string_lossy().into_owned();
        let (opened_tx, opened_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let worker_root = artifact_root_text.clone();
        let worker = std::thread::spawn(move || {
            let guard = ScopedCargoTarget::acquire_with_hook(&worker_root, &scope, || {
                opened_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
            })
            .unwrap();
            acquired_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });
        opened_rx.recv().unwrap();

        let (prune_started_tx, prune_started_rx) = mpsc::channel();
        let (prune_result_tx, prune_result_rx) = mpsc::channel();
        let prune_root = artifact_root_text.clone();
        let pruner = std::thread::spawn(move || {
            prune_started_tx.send(()).unwrap();
            prune_result_tx
                .send(prune_inactive(&prune_root, false))
                .unwrap();
        });
        prune_started_rx.recv().unwrap();
        assert!(
            prune_result_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "pruning must wait while target creation owns the lifecycle lock"
        );

        continue_tx.send(()).unwrap();
        acquired_rx.recv().unwrap();
        assert_eq!(
            prune_result_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            0,
            "the live target must remain protected once creation completes"
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        pruner.join().unwrap();
    }

    #[test]
    fn concurrent_same_identity_crates_execute_their_own_outputs() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_root = tmp.path().join("artifacts");
        let mut runs = Vec::new();

        for identity in ["WORKTREE_A", "WORKTREE_B"] {
            let crate_dir = tmp.path().join(identity);
            fs::create_dir_all(crate_dir.join("src")).unwrap();
            fs::write(
                crate_dir.join("Cargo.toml"),
                "[package]\nname = \"same-package\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
            fs::write(
                crate_dir.join("src/main.rs"),
                format!("fn main() {{ println!(\"{identity}\"); }}\n"),
            )
            .unwrap();
            let session = tmp.path().join("sessions").join(identity);
            let guard =
                ScopedCargoTarget::acquire(artifact_root.to_str().unwrap(), &session).unwrap();
            runs.push((identity.to_string(), crate_dir, guard));
        }

        let threads: Vec<_> = runs
            .into_iter()
            .map(|(identity, crate_dir, guard)| {
                std::thread::spawn(move || {
                    let output = Command::new("cargo")
                        .args(["run", "--quiet"])
                        .current_dir(crate_dir)
                        .env("CARGO_TARGET_DIR", guard.path())
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "cargo run failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), identity);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
