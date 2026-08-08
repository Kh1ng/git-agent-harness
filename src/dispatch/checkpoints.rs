//! Issue #362: Resumable checkpoint management for graceful shutdown recovery.
//!
//! This module provides functionality to:
//! - Detect existing resumable checkpoints for a work item
//! - Create worktrees from checkpoints instead of from target branch
//! - Track checkpoint creation during shutdown
//! - Tombstone consumed checkpoints
//! - Implement retention policies for checkpoint pruning

use crate::ledger::LedgerEntry;
use crate::worktree;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Checkpoint information for resumable work
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    /// The branch/ref where the checkpoint was saved
    pub checkpoint_branch: String,
    /// The SHA of the checkpoint commit
    pub checkpoint_sha: Option<String>,
    /// The attempt number that created this checkpoint
    pub attempt_number: u32,
    /// The work ID this checkpoint belongs to
    pub work_id: Option<String>,
    /// The dispatch branch this checkpoint was created from
    pub dispatch_branch: String,
    /// Timestamp when checkpoint was created (Unix timestamp)
    pub created_timestamp: u64,
    /// Whether this checkpoint is still valid/resumable
    pub is_valid: bool,
}

/// File name for storing checkpoint registry in the GAH state directory
const CHECKPOINT_REGISTRY_FILE: &str = "checkpoint-registry.json";

/// Checkpoint registry tracking all active checkpoints
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CheckpointRegistry {
    /// Map from work_id to list of checkpoints
    pub checkpoints_by_work_id: HashMap<String, Vec<CheckpointInfo>>,
    /// Map from dispatch_branch to latest checkpoint
    pub latest_by_dispatch_branch: HashMap<String, CheckpointInfo>,
}

impl CheckpointRegistry {
    /// Load checkpoint registry from disk
    pub fn load(state_dir: &Path) -> Result<Self> {
        let registry_path = state_dir.join(CHECKPOINT_REGISTRY_FILE);
        if registry_path.exists() {
            let content = fs::read_to_string(&registry_path).with_context(|| {
                format!(
                    "Reading checkpoint registry from {}",
                    registry_path.display()
                )
            })?;
            serde_json::from_str(&content).with_context(|| {
                format!(
                    "Parsing checkpoint registry from {}",
                    registry_path.display()
                )
            })
        } else {
            Ok(Self::default())
        }
    }

    /// Save checkpoint registry to disk
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        let registry_path = state_dir.join(CHECKPOINT_REGISTRY_FILE);
        fs::create_dir_all(state_dir)?;
        let content =
            serde_json::to_string_pretty(self).context("Serializing checkpoint registry")?;
        fs::write(&registry_path, content)
            .with_context(|| format!("Writing checkpoint registry to {}", registry_path.display()))
    }

    /// Register a new checkpoint
    pub fn register_checkpoint(
        &mut self,
        work_id: Option<String>,
        dispatch_branch: &str,
        checkpoint_branch: &str,
        checkpoint_sha: Option<String>,
        attempt_number: u32,
    ) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let checkpoint = CheckpointInfo {
            checkpoint_branch: checkpoint_branch.to_string(),
            checkpoint_sha,
            attempt_number,
            work_id: work_id.clone(),
            dispatch_branch: dispatch_branch.to_string(),
            created_timestamp: timestamp,
            is_valid: true,
        };

        // Add to work_id index
        if let Some(ref work_id) = work_id {
            self.checkpoints_by_work_id
                .entry(work_id.clone())
                .or_default()
                .push(checkpoint.clone());
        }

        // Add to dispatch_branch index (keep only latest)
        self.latest_by_dispatch_branch
            .insert(dispatch_branch.to_string(), checkpoint);
    }

    /// Get latest valid checkpoint for a work item
    pub fn get_latest_checkpoint(&self, work_id: &str) -> Option<&CheckpointInfo> {
        self.checkpoints_by_work_id
            .get(work_id)
            .and_then(|checkpoints| {
                checkpoints
                    .iter()
                    .filter(|cp| cp.is_valid)
                    .max_by_key(|cp| cp.created_timestamp)
            })
    }

    /// Get latest valid checkpoint for a dispatch branch
    pub fn get_latest_checkpoint_for_branch(
        &self,
        dispatch_branch: &str,
    ) -> Option<&CheckpointInfo> {
        self.latest_by_dispatch_branch
            .get(dispatch_branch)
            .filter(|cp| cp.is_valid)
    }

    /// Mark checkpoint as consumed/invalid
    pub fn tombstone_checkpoint(&mut self, checkpoint_branch: &str) {
        // Mark as invalid in work_id index
        for checkpoints in self.checkpoints_by_work_id.values_mut() {
            for checkpoint in checkpoints.iter_mut() {
                if checkpoint.checkpoint_branch == checkpoint_branch {
                    checkpoint.is_valid = false;
                }
            }
        }

        // Mark as invalid in dispatch_branch index
        for checkpoint in self.latest_by_dispatch_branch.values_mut() {
            if checkpoint.checkpoint_branch == checkpoint_branch {
                checkpoint.is_valid = false;
            }
        }
    }

    /// Prune expired checkpoints based on retention policy
    pub fn prune_expired(&mut self, retention_seconds: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut work_ids_to_cleanup = Vec::new();

        // Clean up work_id index - collect checkpoints to invalidate first
        for (work_id, checkpoints) in &mut self.checkpoints_by_work_id {
            let mut cp_is_valid = Vec::new();
            for cp in checkpoints.iter() {
                let age = now.saturating_sub(cp.created_timestamp);
                cp_is_valid.push(age <= retention_seconds || !cp.is_valid);
            }

            // Invalidate checkpoints that are too old
            for (i, should_be_valid) in cp_is_valid.iter().enumerate() {
                if !*should_be_valid {
                    if let Some(cp) = checkpoints.get_mut(i) {
                        cp.is_valid = false;
                    }
                }
            }

            // Remove invalid checkpoints from the list
            checkpoints.retain(|cp| cp.is_valid);

            if checkpoints.is_empty() {
                work_ids_to_cleanup.push(work_id.clone());
            }
        }

        // Remove empty work_id entries
        for work_id in work_ids_to_cleanup {
            self.checkpoints_by_work_id.remove(&work_id);
        }

        // Clean up dispatch_branch index - need to collect keys to remove
        let branches_to_remove: Vec<String> = self
            .latest_by_dispatch_branch
            .iter()
            .filter(|(_, cp)| {
                let age = now.saturating_sub(cp.created_timestamp);
                if age > retention_seconds {
                    // Mark as invalid - we'll do this after collecting
                    true
                } else {
                    false
                }
            })
            .map(|(branch, _)| branch.clone())
            .collect();

        // Remove old entries
        for branch in branches_to_remove {
            if let Some(cp) = self.latest_by_dispatch_branch.get_mut(&branch) {
                cp.is_valid = false;
            }
            self.latest_by_dispatch_branch.remove(&branch);
        }
    }
}

/// Issue #362: Find existing checkpoint branches in the repository for a given dispatch branch.
/// Looks for branches matching the gah-wip/{dispatch_branch}-attempt-{n} pattern.
pub fn find_existing_checkpoints(
    repo: &Path,
    dispatch_branch: &str,
) -> Result<Vec<(String, String)>> {
    // List all branches with the checkpoint prefix
    let output = worktree::git(&["branch", "--list", "gah-wip/*"], repo)?;

    let mut checkpoints = Vec::new();
    let checkpoint_prefix = format!(
        "gah-wip/{}",
        dispatch_branch.trim_start_matches("gah/").replace('/', "-")
    );

    for branch_line in output.lines() {
        let branch = branch_line.trim();
        if branch.starts_with(&checkpoint_prefix) {
            // Extract attempt number from branch name
            if let Some(attempt_part) = branch.split("-attempt-").last() {
                if let Ok(attempt_num) = attempt_part.parse::<u32>() {
                    checkpoints.push((
                        branch.to_string(),
                        format!("{}-attempt-{}", dispatch_branch, attempt_num),
                    ));
                }
            }
        }
    }

    Ok(checkpoints)
}

/// Issue #362: Get the SHA of a checkpoint branch
pub fn get_checkpoint_sha(repo: &Path, checkpoint_branch: &str) -> Result<Option<String>> {
    worktree::git(&["rev-parse", checkpoint_branch], repo)
        .map(|sha| Some(sha.trim().to_string()))
        .or_else(|_| Ok(None))
}

/// Issue #362: Create a worktree from an existing checkpoint branch
pub fn create_worktree_from_checkpoint(
    repo: &Path,
    checkpoint_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    worktree::create_existing(repo, checkpoint_branch, worktree_base)
}

/// Issue #362: validate + resume a checkpoint into a worktree, then
/// re-branch onto `new_branch` (the normal fresh dispatch branch name) so
/// the resumed content doesn't inherit the WIP checkpoint branch's
/// identity -- avoids a name collision if the same checkpoint is ever
/// resumed twice. Returns `Ok(None)` if the checkpoint is no longer valid;
/// the caller falls back to a fresh worktree from the target branch.
pub fn resume_checkpoint_into_worktree(
    ledger: &mut LedgerEntry,
    repo: &Path,
    checkpoint_branch: &str,
    new_branch: &str,
    worktree_base: &Path,
) -> Result<Option<PathBuf>> {
    if !is_valid_checkpoint(repo, checkpoint_branch)? {
        return Ok(None);
    }
    let wt = crate::dispatch::attempts::classify_worktree_result(
        ledger,
        create_worktree_from_checkpoint(repo, checkpoint_branch, worktree_base),
    )?;
    worktree::git(&["checkout", "-b", new_branch], &wt)?;
    Ok(Some(wt))
}

/// Issue #362: Check if a checkpoint exists and is valid for resumption
pub fn is_valid_checkpoint(repo: &Path, checkpoint_branch: &str) -> Result<bool> {
    // Check if branch exists
    let branch_list = worktree::git(&["branch", "--list", checkpoint_branch], repo)?;
    if !branch_list.contains(checkpoint_branch) {
        return Ok(false);
    }

    // Check if the branch has any commits
    let sha_result = worktree::git(&["rev-parse", checkpoint_branch], repo);
    Ok(sha_result.is_ok())
}

/// Issue #362: Record checkpoint information in the ledger entry's typed
/// `resumable_checkpoint_branch`/`resumable_checkpoint_sha` fields. Only
/// sets them when `is_resumable` -- a clean/no-change shutdown must not
/// leave a stale checkpoint identity on the entry.
pub fn record_checkpoint_in_ledger(
    ledger: &mut LedgerEntry,
    checkpoint_branch: &str,
    checkpoint_sha: Option<String>,
    is_resumable: bool,
) {
    if is_resumable {
        ledger.resumable_checkpoint_branch = Some(checkpoint_branch.to_string());
        ledger.resumable_checkpoint_sha = checkpoint_sha;
    }
}

/// Issue #362: Mark an attempt as having a resumable checkpoint, via the
/// typed `AttemptRecord.checkpoint_branch`/`checkpoint_sha` fields rather
/// than appending prose onto `validation_result`.
pub fn mark_attempt_as_resumable(
    ledger: &mut LedgerEntry,
    attempt_number: u32,
    checkpoint_branch: &str,
    checkpoint_sha: Option<String>,
) {
    if let Some(last_attempt) = ledger.attempts.last_mut() {
        if last_attempt.attempt_number == attempt_number {
            last_attempt.checkpoint_branch = Some(checkpoint_branch.to_string());
            last_attempt.checkpoint_sha = checkpoint_sha;
        }
    }
}

/// Issue #362: Default retention policy for checkpoints - 7 days
const DEFAULT_CHECKPOINT_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60; // 7 days

/// Issue #362: Prune checkpoints based on retention policy
/// This should be called periodically to clean up old checkpoints
pub fn prune_checkpoints(
    sessions_base: &Path,
    state_dir: &Path,
    retention_seconds: Option<u64>,
) -> Result<()> {
    let retention_seconds = retention_seconds.unwrap_or(DEFAULT_CHECKPOINT_RETENTION_SECONDS);

    // Load checkpoint registry
    let mut registry = CheckpointRegistry::load(state_dir)?;

    // Prune expired checkpoints from registry
    registry.prune_expired(retention_seconds);

    // Save updated registry
    registry.save(state_dir)?;

    // Also clean up old session directories that contain expired checkpoints
    if sessions_base.exists() && sessions_base.is_dir() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for entry in fs::read_dir(sessions_base)? {
            let entry = entry?;
            let session_path = entry.path();

            if session_path.is_dir() {
                let recovery_file = session_path.join("shutdown-recovery.json");
                if recovery_file.exists() {
                    if let Ok(content) = fs::read_to_string(&recovery_file) {
                        if let Ok(recovery_data) =
                            serde_json::from_str::<serde_json::Value>(&content)
                        {
                            if let Some(timestamp) =
                                recovery_data.get("timestamp").and_then(|v| v.as_u64())
                            {
                                let age = now.saturating_sub(timestamp);
                                if age > retention_seconds {
                                    // Issue #362 review finding: this previously checked
                                    // whether ANY checkpoint anywhere in the registry was
                                    // valid, not whether THIS session's own checkpoint
                                    // branch was -- as long as one unrelated checkpoint
                                    // was still valid, no expired session was ever
                                    // cleaned up, making retention inoperable. Must
                                    // check this specific session's recovery_branch.
                                    let session_branch = recovery_data
                                        .get("recovery_branch")
                                        .and_then(|v| v.as_str());
                                    let is_referenced =
                                        session_branch.is_some_and(|branch| {
                                            registry.checkpoints_by_work_id.values().flatten().any(
                                                |cp| cp.checkpoint_branch == branch && cp.is_valid,
                                            ) || registry.latest_by_dispatch_branch.values().any(
                                                |cp| cp.checkpoint_branch == branch && cp.is_valid,
                                            )
                                        });

                                    if !is_referenced {
                                        // Safe to clean up this session
                                        fs::remove_dir_all(&session_path)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Issue #362: Find the latest resumable checkpoint for a work item by scanning
/// session directories and shutdown-recovery.json files
pub fn find_latest_resumable_checkpoint(
    sessions_base: &Path,
    dispatch_branch: &str,
    work_id: Option<&str>,
) -> Result<Option<(String, String, String)>> {
    // Look for session directories that might contain checkpoints
    // The session directories are typically named like: gah-<work-id>-<timestamp>
    // or similar patterns

    let mut latest_checkpoint: Option<(String, String, String)> = None; // (branch, sha, timestamp)

    if sessions_base.exists() && sessions_base.is_dir() {
        for entry in fs::read_dir(sessions_base)? {
            let entry = entry?;
            let session_path = entry.path();

            if session_path.is_dir() {
                let recovery_file = session_path.join("shutdown-recovery.json");
                if recovery_file.exists() {
                    if let Ok(content) = fs::read_to_string(&recovery_file) {
                        if let Ok(recovery_data) =
                            serde_json::from_str::<serde_json::Value>(&content)
                        {
                            // Check if this recovery is for our dispatch branch or work id
                            let matches_work_id = match work_id {
                                Some(wid) => {
                                    recovery_data.get("source_work_id").and_then(|v| v.as_str())
                                        == Some(wid)
                                }
                                None => true,
                            };

                            let matches_dispatch_branch = {
                                recovery_data
                                    .get("dispatch_branch")
                                    .and_then(|v| v.as_str())
                                    == Some(dispatch_branch)
                            };

                            if matches_work_id && matches_dispatch_branch {
                                if let (Some(branch), Some(sha), Some(timestamp)) = (
                                    recovery_data
                                        .get("recovery_branch")
                                        .and_then(|v| v.as_str()),
                                    recovery_data.get("checkpoint_sha").and_then(|v| v.as_str()),
                                    recovery_data.get("timestamp").and_then(|v| v.as_u64()),
                                ) {
                                    // Check if this is the latest checkpoint
                                    latest_checkpoint = match latest_checkpoint {
                                        None => Some((
                                            branch.to_string(),
                                            sha.to_string(),
                                            timestamp.to_string(),
                                        )),
                                        Some((
                                            existing_branch,
                                            existing_sha,
                                            existing_timestamp,
                                        )) => {
                                            if timestamp
                                                > existing_timestamp.parse::<u64>().unwrap_or(0)
                                            {
                                                Some((
                                                    branch.to_string(),
                                                    sha.to_string(),
                                                    timestamp.to_string(),
                                                ))
                                            } else {
                                                Some((
                                                    existing_branch,
                                                    existing_sha,
                                                    existing_timestamp,
                                                ))
                                            }
                                        }
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(latest_checkpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_checkpoint_registry_roundtrip() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path();

        let mut registry = CheckpointRegistry::default();
        registry.register_checkpoint(
            Some("test-work-id".to_string()),
            "gah/test-123",
            "gah-wip/gah-test-123-attempt-1",
            Some("abc123".to_string()),
            1,
        );

        registry.save(state_dir).unwrap();
        let loaded = CheckpointRegistry::load(state_dir).unwrap();

        assert!(loaded.checkpoints_by_work_id.contains_key("test-work-id"));
        assert!(loaded
            .latest_by_dispatch_branch
            .contains_key("gah/test-123"));
    }

    #[test]
    fn test_checkpoint_registry_prune_expired() {
        let _dir = tempdir().unwrap();

        let mut registry = CheckpointRegistry::default();

        // Add a recent checkpoint (should not be pruned)
        registry.register_checkpoint(
            Some("recent-work".to_string()),
            "gah/recent-123",
            "gah-wip/gah-recent-123-attempt-1",
            Some("recent123".to_string()),
            1,
        );

        // Add an old checkpoint (should be pruned)
        let old_checkpoint = CheckpointInfo {
            checkpoint_branch: "gah-wip/gah-old-123-attempt-1".to_string(),
            checkpoint_sha: Some("old123".to_string()),
            attempt_number: 1,
            work_id: Some("old-work".to_string()),
            dispatch_branch: "gah/old-123".to_string(),
            created_timestamp: 1000, // Very old timestamp
            is_valid: true,
        };
        registry
            .checkpoints_by_work_id
            .entry("old-work".to_string())
            .or_default()
            .push(old_checkpoint.clone());
        registry
            .latest_by_dispatch_branch
            .insert("gah/old-123".to_string(), old_checkpoint);

        // Prune with a short retention period
        registry.prune_expired(2000); // 2000 seconds

        // Recent checkpoint should still be valid
        assert!(registry.checkpoints_by_work_id.contains_key("recent-work"));
        assert!(registry
            .latest_by_dispatch_branch
            .contains_key("gah/recent-123"));

        // Old checkpoint should be marked invalid
        let old_entry = registry.checkpoints_by_work_id.get("old-work");
        if let Some(checkpoints) = old_entry {
            for cp in checkpoints {
                assert!(!cp.is_valid, "Old checkpoint should be marked as invalid");
            }
        }
    }

    #[test]
    fn test_find_latest_resumable_checkpoint() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir().unwrap();
        let sessions_base = dir.path();

        // Create a fake session directory with shutdown-recovery.json
        let session_dir = sessions_base.join("gah-test-123-20260807");
        fs::create_dir_all(&session_dir).unwrap();

        // Create a recovery file with old timestamp
        let old_recovery = serde_json::json!({
            "run_id": "run1",
            "attempt_number": 1,
            "source_work_id": "test-123",
            "dispatch_branch": "gah/test-123",
            "recovery_branch": "gah-wip/gah-test-123-attempt-1",
            "checkpointed": true,
            "checkpoint_sha": "abc123",
            "is_resumable": true,
            "timestamp": 1000
        });
        fs::write(
            session_dir.join("shutdown-recovery.json"),
            serde_json::to_string(&old_recovery)?,
        )
        .unwrap();

        // Create another session with newer timestamp
        let session_dir2 = sessions_base.join("gah-test-123-20260808");
        fs::create_dir_all(&session_dir2).unwrap();

        let new_recovery = serde_json::json!({
            "run_id": "run2",
            "attempt_number": 2,
            "source_work_id": "test-123",
            "dispatch_branch": "gah/test-123",
            "recovery_branch": "gah-wip/gah-test-123-attempt-2",
            "checkpointed": true,
            "checkpoint_sha": "def456",
            "is_resumable": true,
            "timestamp": 2000
        });
        fs::write(
            session_dir2.join("shutdown-recovery.json"),
            serde_json::to_string(&new_recovery)?,
        )
        .unwrap();

        // Test finding the latest checkpoint
        let result =
            find_latest_resumable_checkpoint(sessions_base, "gah/test-123", Some("test-123"))
                .unwrap();

        assert!(result.is_some());
        if let Some((branch, sha, _)) = result {
            assert_eq!(branch, "gah-wip/gah-test-123-attempt-2");
            assert_eq!(sha, "def456");
        }
        Ok(())
    }

    /// Issue #362 review finding: `prune_checkpoints`'s session cleanup
    /// previously checked whether ANY checkpoint anywhere in the registry
    /// was valid, not whether THIS session's own checkpoint branch was --
    /// meaning an unrelated still-valid checkpoint protected every expired
    /// session from cleanup, making retention effectively inoperable.
    #[test]
    fn prune_checkpoints_removes_expired_session_even_when_an_unrelated_checkpoint_is_valid(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let sessions_dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();

        // An expired session whose own checkpoint branch is nowhere in the
        // registry at all (the common real case: it was already consumed/
        // tombstoned, so the registry has no entry for it).
        let expired_session = sessions_dir.path().join("gah-expired-20260101");
        fs::create_dir_all(&expired_session).unwrap();
        fs::write(
            expired_session.join("shutdown-recovery.json"),
            serde_json::to_string(&serde_json::json!({
                "recovery_branch": "gah-wip/expired-attempt-1",
                "checkpoint_sha": "expired123",
                "timestamp": 1000,
            }))?,
        )
        .unwrap();

        // An unrelated, still-valid checkpoint for a completely different
        // dispatch -- this must not protect the expired session above.
        let mut registry = CheckpointRegistry::default();
        registry.register_checkpoint(
            Some("other-work".to_string()),
            "gah/other-123",
            "gah-wip/other-attempt-1",
            Some("other456".to_string()),
            1,
        );
        registry.save(state_dir.path())?;

        prune_checkpoints(sessions_dir.path(), state_dir.path(), Some(2000))?;

        assert!(
            !expired_session.exists(),
            "expired session must be cleaned up even though an unrelated checkpoint is still valid"
        );
        Ok(())
    }
}
