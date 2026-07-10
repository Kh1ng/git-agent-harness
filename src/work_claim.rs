use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Work claim state for tracking in-flight work IDs per profile
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WorkClaimState {
    pub version: u32,
    pub claims: std::collections::HashMap<String, Vec<String>>, // profile -> work_ids
}

impl WorkClaimState {
    pub fn new() -> Self {
        Self {
            version: 1,
            claims: std::collections::HashMap::new(),
        }
    }

    /// Claim a work_id for a profile
    pub fn claim(&mut self, profile: &str, work_id: &str) {
        self.claims
            .entry(profile.to_string())
            .or_default()
            .push(work_id.to_string());
    }

    /// Release a work_id for a profile
    pub fn release(&mut self, profile: &str, work_id: &str) {
        if let Some(claims) = self.claims.get_mut(profile) {
            claims.retain(|id| id != work_id);
        }
    }

    /// Get all claimed work_ids for a profile
    pub fn get_claimed(&self, profile: &str) -> Vec<String> {
        self.claims.get(profile).cloned().unwrap_or_default()
    }

    /// Check if a work_id is claimed for a profile
    pub fn is_claimed(&self, profile: &str, work_id: &str) -> bool {
        self.claims
            .get(profile)
            .map(|claims| claims.contains(&work_id.to_string()))
            .unwrap_or(false)
    }
}

/// Path to the work claims file
fn work_claims_path() -> PathBuf {
    resolve_state_path_from_env(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

fn work_claims_lock_path() -> PathBuf {
    let mut path = work_claims_path();
    path.set_extension("json.lock");
    path
}

fn with_locked_state<T>(update: impl FnOnce(&mut WorkClaimState) -> Result<T>) -> Result<T> {
    let path = work_claims_path();
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).with_context(|| format!("creating state dir: {:?}", parent))?;
    let lock_path = work_claims_lock_path();
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening claim lock: {:?}", lock_path))?;
    lock.lock_exclusive().context("locking work claims")?;
    let mut state = load_state()?;
    let result = update(&mut state)?;
    save_state(&state)?;
    FileExt::unlock(&lock).ok();
    Ok(result)
}

/// Resolve the state file path from explicit env values. Pure function (no
/// direct env reads) so path-resolution tests never touch process-global
/// environment — see the PATH-mutation lesson from provider.rs's test seam.
fn resolve_state_path_from_env(xdg_state_home: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(xdg) = xdg_state_home.filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("gah").join("work_claims.json");
    }
    let home = home.unwrap_or("/root");
    PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("gah")
        .join("work_claims.json")
}

/// Load work claim state from file
fn load_state() -> Result<WorkClaimState> {
    let path = work_claims_path();
    if !path.exists() {
        return Ok(WorkClaimState::new());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read work claims file: {:?}", path))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse work claims file: {:?}", path))
}

/// Save work claim state to file
fn save_state(state: &WorkClaimState) -> Result<()> {
    let path = work_claims_path();
    let parent = path.parent().unwrap();
    if !parent.exists() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create state dir: {:?}", parent))?;
    }
    let content = serde_json::to_string_pretty(state)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("Failed to open work claims file for writing: {:?}", path))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write work claims file: {:?}", path))?;
    Ok(())
}

/// Claim a work_id for a profile and persist to file
/// Atomically claim work across independent GAH processes.
pub fn try_claim_work(profile: &str, work_id: &str) -> Result<bool> {
    with_locked_state(|state| {
        if state.is_claimed(profile, work_id) {
            return Ok(false);
        }
        state.claim(profile, work_id);
        Ok(true)
    })
}

/// Release all claims for a profile (useful for cleanup)
pub fn release_all_for_profile(profile: &str) -> Result<()> {
    with_locked_state(|state| {
        state.claims.remove(profile);
        Ok(())
    })
}

/// Release a work_id for a profile and persist to file  
pub fn release_work(profile: &str, work_id: &str) -> Result<()> {
    with_locked_state(|state| {
        state.release(profile, work_id);
        Ok(())
    })
}

/// Get all claimed work_ids for a profile
pub fn get_claimed_work_ids(profile: &str) -> Result<Vec<String>> {
    with_locked_state(|state| Ok(state.get_claimed(profile)))
}

/// Check if a work_id is claimed for a profile
pub fn is_claimed(profile: &str, work_id: &str) -> Result<bool> {
    with_locked_state(|state| Ok(state.is_claimed(profile, work_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_work_claim_state_claim_and_release() {
        let mut state = WorkClaimState::new();

        // Test initial state
        assert!(!state.is_claimed("test_profile", "work_1"));
        assert!(state.get_claimed("test_profile").is_empty());

        // Test claiming work
        state.claim("test_profile", "work_1");
        assert!(state.is_claimed("test_profile", "work_1"));
        assert_eq!(state.get_claimed("test_profile"), vec!["work_1"]);

        // Test claiming another work
        state.claim("test_profile", "work_2");
        assert!(state.is_claimed("test_profile", "work_2"));
        assert_eq!(state.get_claimed("test_profile"), vec!["work_1", "work_2"]);

        // Test releasing work
        state.release("test_profile", "work_1");
        assert!(!state.is_claimed("test_profile", "work_1"));
        assert!(state.is_claimed("test_profile", "work_2"));
        assert_eq!(state.get_claimed("test_profile"), vec!["work_2"]);

        // Test releasing non-existent work (should be no-op)
        state.release("test_profile", "nonexistent");
        assert_eq!(state.get_claimed("test_profile"), vec!["work_2"]);
    }

    #[test]
    fn test_work_claim_state_multiple_profiles() {
        let mut state = WorkClaimState::new();

        state.claim("profile1", "work_1");
        state.claim("profile2", "work_1"); // Same work_id, different profile

        assert!(state.is_claimed("profile1", "work_1"));
        assert!(state.is_claimed("profile2", "work_1"));

        assert_eq!(state.get_claimed("profile1"), vec!["work_1"]);
        assert_eq!(state.get_claimed("profile2"), vec!["work_1"]);

        // Releasing from one profile shouldn't affect the other
        state.release("profile1", "work_1");
        assert!(!state.is_claimed("profile1", "work_1"));
        assert!(state.is_claimed("profile2", "work_1"));
    }

    #[test]
    fn test_work_claim_state_clear_profile() {
        let mut state = WorkClaimState::new();

        state.claim("test_profile", "work_1");
        state.claim("test_profile", "work_2");
        state.claim("other_profile", "work_3");

        // Clear all claims for test_profile
        state.claims.remove("test_profile");

        assert!(!state.is_claimed("test_profile", "work_1"));
        assert!(!state.is_claimed("test_profile", "work_2"));
        assert!(state.is_claimed("other_profile", "work_3"));
    }
}
