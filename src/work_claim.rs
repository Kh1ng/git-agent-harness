use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

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
            .or_insert_with(Vec::new)
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

/// Global in-memory state with file persistence
static STATE: OnceLock<Mutex<WorkClaimState>> = OnceLock::new();

/// Path to the work claims file
fn work_claims_path() -> PathBuf {
    resolve_state_path_from_env(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
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

/// Initialize the global state
fn init_state() -> Result<()> {
    let state = load_state()?;
    let global = STATE.get_or_init(|| Mutex::new(WorkClaimState::new()));
    let mut guard = global
        .lock()
        .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
    *guard = state;
    Ok(())
}

/// Get the global work claim state
fn get_state() -> Result<MutexGuard<'static, WorkClaimState>> {
    init_state()?;
    STATE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Work claim state not initialized"))?
        .lock()
        .map_err(|_| anyhow::anyhow!("Mutex poisoned"))
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
pub fn claim_work(profile: &str, work_id: &str) -> Result<()> {
    let mut state = get_state()?;
    state.claim(profile, work_id);
    save_state(&state)?;
    Ok(())
}

/// Release a work_id for a profile and persist to file  
pub fn release_work(profile: &str, work_id: &str) -> Result<()> {
    let mut state = get_state()?;
    state.release(profile, work_id);
    save_state(&state)?;
    Ok(())
}

/// Get all claimed work_ids for a profile
pub fn get_claimed_work_ids(profile: &str) -> Result<Vec<String>> {
    let state = get_state()?;
    Ok(state.get_claimed(profile))
}

/// Check if a work_id is claimed for a profile
pub fn is_claimed(profile: &str, work_id: &str) -> Result<bool> {
    let state = get_state()?;
    Ok(state.is_claimed(profile, work_id))
}

/// Release all claims for a profile (useful for cleanup)
pub fn release_all_for_profile(profile: &str) -> Result<()> {
    let mut state = get_state()?;
    state.claims.remove(profile);
    save_state(&state)?;
    Ok(())
}
