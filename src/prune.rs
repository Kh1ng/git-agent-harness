use crate::config::{self, GahConfig, Profile};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn run(
    profile_name: Option<&str>,
    config_path: Option<&str>,
    older_than_days: u64,
    dry_run: bool,
) -> Result<()> {
    let cfg = config::load(config_path)?;
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(older_than_days.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let profiles = selected_profiles(&cfg, profile_name)?;

    for (name, profile) in profiles {
        println!("[{}]", name);
        prune_sessions(profile, cutoff, dry_run)?;
        prune_worktrees(&cfg, profile, cutoff, dry_run)?;
    }
    Ok(())
}

fn selected_profiles<'a>(
    cfg: &'a GahConfig,
    profile_name: Option<&str>,
) -> Result<Vec<(String, &'a Profile)>> {
    if let Some(name) = profile_name {
        return Ok(vec![(name.to_string(), config::get_profile(cfg, name)?)]);
    }
    let mut profiles: Vec<_> = cfg.profiles.iter().map(|(k, v)| (k.clone(), v)).collect();
    profiles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(profiles)
}

fn prune_sessions(profile: &Profile, cutoff: SystemTime, dry_run: bool) -> Result<()> {
    let root = Path::new(&profile.artifact_root).join("sessions");
    if !root.exists() {
        println!("  sessions: none");
        return Ok(());
    }
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if !is_older_than(&path, cutoff) {
            continue;
        }
        if dry_run {
            println!("  would remove session {}", path.display());
        } else if path.is_dir() {
            fs::remove_dir_all(&path)?;
            println!("  removed session {}", path.display());
        } else {
            fs::remove_file(&path)?;
            println!("  removed file {}", path.display());
        }
    }
    Ok(())
}

fn prune_worktrees(
    cfg: &GahConfig,
    profile: &Profile,
    cutoff: SystemTime,
    dry_run: bool,
) -> Result<()> {
    if cfg.defaults.worktree_base.trim().is_empty() {
        println!("  worktrees: skipped (no defaults.worktree_base)");
        return Ok(());
    }
    let root = Path::new(&cfg.defaults.worktree_base);
    if !root.exists() {
        println!("  worktrees: none");
        return Ok(());
    }
    let prefixes = [
        format!("gah-{}-", profile.repo_id),
        format!("gah-exp-{}-", profile.repo_id),
    ];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) || !is_older_than(&path, cutoff)
        {
            continue;
        }
        if dry_run {
            println!("  would remove worktree {}", path.display());
        } else {
            let _ = crate::worktree::git(
                &["worktree", "remove", "-f", path.to_str().unwrap_or("")],
                Path::new(&profile.local_path),
            );
            println!("  removed worktree {}", path.display());
        }
    }
    if !dry_run {
        let _ = crate::worktree::git(&["worktree", "prune"], Path::new(&profile.local_path));
    }
    Ok(())
}

fn is_older_than(path: &PathBuf, cutoff: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|modified| modified <= cutoff)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::is_older_than;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    #[test]
    fn older_than_uses_file_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = PathBuf::from(tmp.path()).join("old");
        fs::write(&path, "x").unwrap();
        std::process::Command::new("touch")
            .args(["-t", "202401010000", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(is_older_than(
            &path,
            SystemTime::now()
                .checked_sub(Duration::from_secs(86_400))
                .unwrap()
        ));
    }
}
