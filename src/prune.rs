use crate::config::{self, GahConfig, Profile};
use crate::sync::{self, SyncMr};
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

pub fn run(
    profile_name: Option<&str>,
    config_path: Option<&str>,
    older_than_days: Option<u64>,
    dry_run: bool,
) -> Result<()> {
    let cfg = config::load(config_path)?;
    let profiles = selected_profiles(&cfg, profile_name)?;

    for (name, profile) in profiles {
        prune_profile(&cfg, &name, profile, older_than_days, dry_run, true)?;
    }
    Ok(())
}

/// Run the same conservative maintenance used by `gah prune` for one
/// controller profile. The controller calls this before each observation so
/// merged/closed worktrees cannot accumulate until an operator remembers to
/// run a separate command.
pub fn run_automatic(cfg: &GahConfig, profile_name: &str) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    prune_profile(cfg, profile_name, profile, None, false, false)
}

fn prune_profile(
    cfg: &GahConfig,
    name: &str,
    profile: &Profile,
    older_than_days: Option<u64>,
    dry_run: bool,
    announce: bool,
) -> Result<()> {
    // CLI --older-than overrides the per-profile retention window.
    let retention = older_than_days.unwrap_or_else(|| profile.effective_prune_older_than_days());
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if announce {
        println!("[{name}] retention={retention}d");
    }
    prune_sessions(profile, cutoff, dry_run)?;
    crate::build_cache::prune_inactive(&profile.artifact_root, dry_run)?;
    prune_worktrees(cfg, profile, cutoff, dry_run)?;
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

/// Map a PR/MR branch name to the on-disk worktree directory name. Mirrors
/// `worktree::create` / `worktree::create_existing`, which call
/// `worktree_base.join(branch.replace('/', "-"))`. Pure and testable.
pub(crate) fn worktree_name_for_branch(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Returns the set of worktree directory names that correspond to branches
/// whose PR/MR is MERGED or CLOSED_UNMERGED. These worktrees are safe to prune
/// immediately (they are already terminal upstream), regardless of the
/// retention window. Best-effort: any fetch error yields an empty set so pruning
/// never blocks on transient provider/network failure.
fn done_worktree_names(profile: &Profile) -> HashSet<String> {
    match sync::fetch_mrs(profile) {
        Ok(mrs) => done_worktree_names_from_mrs(&mrs),
        Err(_) => HashSet::new(),
    }
}

/// Pure subset of `done_worktree_names`: maps a list of fetched PRs/MRs to the
/// set of on-disk worktree names that are safe to prune immediately.
pub(crate) fn done_worktree_names_from_mrs(mrs: &[SyncMr]) -> HashSet<String> {
    let mut done = HashSet::new();
    for mr in mrs {
        let class = sync::classify(mr);
        if class == "MERGED" || class == "CLOSED_UNMERGED" {
            // Worktree directory names are the branch name with '/' replaced by '-'
            // (see worktree::create / worktree::create_existing). The prune prefix
            // filter already scopes results to this profile's `gah-{repo_id}-` /
            // `gah-exp-{repo_id}-` names, so the bare transformed branch is enough.
            done.insert(worktree_name_for_branch(&mr.branch));
        }
    }
    done
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
    let done = done_worktree_names(profile);
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        // A worktree tied to a merged/closed PR/MR is safe to prune
        // immediately, even if it is newer than the retention cutoff. Local
        // branch topology is deliberately not enough: a fresh worktree has a
        // default-branch tip until its agent makes the first commit, so using
        // that signal would race an in-flight manual dispatch.
        let is_done = done.contains(name.as_str());
        let is_old = is_older_than(&path, cutoff);
        if !is_done && !is_old {
            continue;
        }
        // Never force-remove a dirty worktree. A terminal PR can still have
        // local, unpublished recovery work, and a failed worker can leave a
        // useful patch behind. This is intentionally fail-closed: if Git
        // cannot establish cleanliness, retain the worktree for inspection.
        if !worktree_is_clean(&path) {
            println!("  retained dirty worktree {}", path.display());
            continue;
        }
        if dry_run {
            println!(
                "  would remove worktree {}",
                if is_done {
                    format!("{} (MERGED/CLOSED)", path.display())
                } else {
                    path.display().to_string()
                }
            );
        } else {
            let _ = crate::worktree::git(
                &["worktree", "remove", "-f", path.to_str().unwrap_or("")],
                Path::new(&profile.local_path),
            );
            println!(
                "  removed worktree {}",
                if is_done {
                    format!("{} (MERGED/CLOSED)", path.display())
                } else {
                    path.display().to_string()
                }
            );
        }
    }
    if !dry_run {
        let _ = crate::worktree::git(&["worktree", "prune"], Path::new(&profile.local_path));
    }
    Ok(())
}

fn worktree_is_clean(worktree: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree)
        .output()
        .map(|out| out.status.success() && out.stdout.is_empty())
        .unwrap_or(false)
}

fn is_older_than(path: &PathBuf, cutoff: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|modified| modified <= cutoff)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{done_worktree_names_from_mrs, is_older_than, worktree_name_for_branch};
    use crate::sync::SyncMr;
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

    #[test]
    fn worktree_name_mirrors_branch_slash_replacement() {
        assert_eq!(worktree_name_for_branch("gah/real-123"), "gah-real-123");
        assert_eq!(worktree_name_for_branch("feature/fix"), "feature-fix");
    }

    fn mr(branch: &str, merged: bool, state: Option<&str>) -> SyncMr {
        SyncMr {
            title: String::new(),
            body: None,
            branch: branch.to_string(),
            labels: Vec::new(),
            url: None,
            id: None,
            state: state.map(|s| s.to_string()),
            draft: false,
            source_sha: None,
            merge_status: None,
            merged,
            updated_at: None,
            merged_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: None,
        }
    }

    #[test]
    fn done_set_covers_merged_and_closed_unmerged_only() {
        let mrs = vec![
            mr("gah/real-1", true, None),            // MERGED
            mr("gah/real-2", false, Some("closed")), // CLOSED_UNMERGED
            mr("gah/real-3", false, Some("open")),   // open -> not done
            mr("gah/real-4", false, None),           // open -> not done
        ];
        let done = done_worktree_names_from_mrs(&mrs);
        assert!(done.contains("gah-real-1"));
        assert!(done.contains("gah-real-2"));
        assert!(!done.contains("gah-real-3"));
        assert!(!done.contains("gah-real-4"));
    }
}
