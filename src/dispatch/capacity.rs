use crate::config::Profile;
use anyhow::{Context, Result};
use std::path::Path;

const MIN_DISPATCH_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub(super) fn ensure_dispatch_capacity(profile: &Profile, worktree_base: &Path) -> Result<()> {
    ensure_minimum_free_space(worktree_base, "worktree filesystem")?;
    ensure_minimum_free_space(&std::env::temp_dir(), "temporary filesystem")?;
    // Ensure the isolated-target parent exists before the first backend
    // inherits it; this also proves the configured artifact root is writable
    // early without sharing mutable Cargo outputs between worktrees.
    std::fs::create_dir_all(crate::build_cache::target_root(&profile.artifact_root))
        .context("creating isolated Cargo target root")?;
    Ok(())
}

fn ensure_minimum_free_space(path: &Path, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        // Dispatch creates the worktree directory after this preflight.  A
        // configured worktree base therefore commonly does not exist yet;
        // stat the nearest existing ancestor to measure the same filesystem
        // without making a harmless first dispatch fail with ENOENT.
        let filesystem_path = nearest_existing_ancestor(path)?;
        let path_c = CString::new(filesystem_path.as_os_str().as_bytes())?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path_c.as_ptr(), &mut stat) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "checking free space for {} (configured path {}; filesystem path {})",
                    label,
                    path.display(),
                    filesystem_path.display(),
                )
            });
        }
        let available = (stat.f_bavail as u128).saturating_mul(stat.f_frsize as u128);
        if available < MIN_DISPATCH_FREE_BYTES as u128 {
            anyhow::bail!(
                "insufficient free space on {} ({}): {} GiB available; require at least {} GiB before dispatch",
                label,
                path.display(),
                available / (1024 * 1024 * 1024),
                MIN_DISPATCH_FREE_BYTES / (1024 * 1024 * 1024),
            );
        }
    }
    #[cfg(not(unix))]
    let _ = (path, label);
    Ok(())
}

pub(super) fn nearest_existing_ancestor(path: &Path) -> Result<&Path> {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| anyhow::anyhow!("no existing ancestor for path {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::nearest_existing_ancestor;

    #[test]
    fn capacity_preflight_uses_existing_parent_for_new_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree_base = tmp.path().join("worktrees");

        assert!(!worktree_base.exists());
        assert_eq!(
            nearest_existing_ancestor(&worktree_base).unwrap(),
            tmp.path()
        );
    }
}
