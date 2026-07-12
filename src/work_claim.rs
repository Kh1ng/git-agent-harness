use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use hostname::get;
use serde::{Deserialize, Serialize};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Individual work claim with ownership and timing metadata
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkClaim {
    /// The work ID being claimed
    pub work_id: String,
    /// Process ID of the claiming process
    pub pid: u32,
    /// Hostname where the claiming process is running
    pub hostname: String,
    /// Timestamp when the claim was made
    pub claimed_at: DateTime<Utc>,
}

/// Work claim state for tracking in-flight work IDs per profile
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WorkClaimState {
    pub version: u32,
    /// Profile -> Vec<WorkClaim> for version 2, Profile -> Vec<work_id> for version 1
    pub claims: std::collections::HashMap<String, Vec<WorkClaimStateEntry>>,
}

/// Entry in the claims map - can be either a v1 string or v2 WorkClaim
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum WorkClaimStateEntry {
    /// Version 1: just the work_id string
    V1(String),
    /// Version 2: full claim with metadata
    V2(WorkClaim),
}

impl WorkClaimState {
    pub fn new() -> Self {
        Self {
            version: 2,
            claims: std::collections::HashMap::new(),
        }
    }

    /// Migrate v1 state to v2 format
    pub fn migrate_from_v1(&mut self) {
        if self.version == 1 {
            let old_claims: std::collections::HashMap<String, Vec<String>> = serde_json::from_str(
                &serde_json::to_string(&self.claims).unwrap_or("{}".to_string()),
            )
            .unwrap_or_default();

            let mut new_claims = std::collections::HashMap::new();
            for (profile, work_ids) in old_claims {
                let claims = work_ids
                    .into_iter()
                    .map(|work_id| {
                        WorkClaimStateEntry::V2(WorkClaim {
                            work_id,
                            pid: 0, // Unknown PID for migrated v1 claims
                            hostname: "unknown".to_string(),
                            claimed_at: Utc::now(),
                        })
                    })
                    .collect();
                new_claims.insert(profile, claims);
            }
            self.claims = new_claims;
            self.version = 2;
        }
    }

    /// Ensure state is v2 format
    pub fn ensure_v2(&mut self) {
        if self.version < 2 {
            self.migrate_from_v1();
        }
    }

    /// Claim a work_id for a profile
    pub fn claim(&mut self, profile: &str, work_id: &str) {
        self.ensure_v2();
        let claim = WorkClaim {
            work_id: work_id.to_string(),
            pid: std::process::id(),
            hostname: get().unwrap_or_default().to_string_lossy().into_owned(),
            claimed_at: Utc::now(),
        };
        self.claims
            .entry(profile.to_string())
            .or_default()
            .push(WorkClaimStateEntry::V2(claim));
    }

    /// Release a work_id for a profile
    pub fn release(&mut self, profile: &str, work_id: &str) {
        self.ensure_v2();
        if let Some(claims) = self.claims.get_mut(profile) {
            claims.retain(|entry| match entry {
                WorkClaimStateEntry::V1(id) => id != work_id,
                WorkClaimStateEntry::V2(claim) => claim.work_id != work_id,
            });
        }
    }

    /// Get all claimed work_ids for a profile
    pub fn get_claimed(&self, profile: &str) -> Vec<String> {
        self.claims
            .get(profile)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| match entry {
                        WorkClaimStateEntry::V1(id) => id.clone(),
                        WorkClaimStateEntry::V2(claim) => claim.work_id.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if a work_id is claimed for a profile
    pub fn is_claimed(&self, profile: &str, work_id: &str) -> bool {
        self.claims
            .get(profile)
            .map(|entries| {
                entries.iter().any(|entry| match entry {
                    WorkClaimStateEntry::V1(id) => id == work_id,
                    WorkClaimStateEntry::V2(claim) => claim.work_id == work_id,
                })
            })
            .unwrap_or(false)
    }

    /// Check if a claim is stale (process dead or too old)
    pub fn is_claim_stale(&self, profile: &str, work_id: &str, max_age_secs: u64) -> bool {
        if let Some(entries) = self.claims.get(profile) {
            for entry in entries {
                match entry {
                    WorkClaimStateEntry::V1(_) => {
                        // v1 claims are considered stale if they exist (no ownership info)
                        return true;
                    }
                    WorkClaimStateEntry::V2(claim) => {
                        if claim.work_id == work_id {
                            // Check if process is still alive
                            if claim.pid == 0 {
                                return true; // Unknown/migrated claim
                            }

                            // Check process liveness
                            if !is_process_alive(claim.pid) {
                                return true;
                            }

                            // Check age
                            let age = Utc::now()
                                .signed_duration_since(claim.claimed_at)
                                .num_seconds() as u64;
                            if age > max_age_secs {
                                return true;
                            }

                            return false;
                        }
                    }
                }
            }
        }
        false
    }

    /// Reclaim stale claims for a profile
    pub fn reclaim_stale_claims(&mut self, profile: &str, max_age_secs: u64) -> Vec<String> {
        self.ensure_v2();
        let mut reclaimed = Vec::new();

        if let Some(claims) = self.claims.get_mut(profile) {
            let mut i = 0;
            while i < claims.len() {
                let is_stale = match &claims[i] {
                    WorkClaimStateEntry::V1(_) => true, // Always reclaim v1 claims
                    WorkClaimStateEntry::V2(claim) => {
                        // Check process liveness
                        if claim.pid == 0 || !is_process_alive(claim.pid) {
                            true
                        } else {
                            // Check age
                            let age = Utc::now()
                                .signed_duration_since(claim.claimed_at)
                                .num_seconds() as u64;
                            age > max_age_secs
                        }
                    }
                };

                if is_stale {
                    if let WorkClaimStateEntry::V2(claim) = &claims[i] {
                        reclaimed.push(claim.work_id.clone());
                    }
                    claims.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        reclaimed
    }

    /// Get all claims with details for a profile
    pub fn get_claims_with_details(&self, profile: &str) -> Vec<ClaimDetail> {
        self.claims
            .get(profile)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| match entry {
                        WorkClaimStateEntry::V1(id) => ClaimDetail {
                            work_id: id.clone(),
                            pid: 0,
                            hostname: "unknown".to_string(),
                            claimed_at: Utc::now(),
                            is_stale: true,
                        },
                        WorkClaimStateEntry::V2(claim) => {
                            let is_stale = claim.pid == 0 || !is_process_alive(claim.pid);
                            ClaimDetail {
                                work_id: claim.work_id.clone(),
                                pid: claim.pid,
                                hostname: claim.hostname.clone(),
                                claimed_at: claim.claimed_at,
                                is_stale,
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Detailed claim information for display
#[derive(Debug, Clone, Serialize)]
pub struct ClaimDetail {
    pub work_id: String,
    pub pid: u32,
    pub hostname: String,
    pub claimed_at: DateTime<Utc>,
    pub is_stale: bool,
}

/// Check if a process is still alive
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .is_ok()
    }

    #[cfg(not(unix))]
    {
        // On non-Unix systems, we can't easily check process liveness
        // So we'll consider the claim not stale based on process check
        true
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
        state.ensure_v2();

        // Check if there's an existing claim and if it's stale
        if state.is_claimed(profile, work_id) {
            if state.is_claim_stale(profile, work_id, 3600) {
                // 1 hour max age
                // Reclaim the stale claim
                state.release(profile, work_id);
            } else {
                return Ok(false); // Claim is active
            }
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

/// Handle `gah claims list` command
pub fn handle_claims_list(profile: Option<&str>, json: bool) -> Result<()> {
    with_locked_state(|state| {
        state.ensure_v2();

        if let Some(profile) = profile {
            // List claims for specific profile
            let claims = state.get_claims_with_details(profile);
            if json {
                println!("{}", serde_json::to_string(&claims)?);
            } else if claims.is_empty() {
                println!("No claims for profile {}", profile);
            } else {
                println!("Claims for profile {}:", profile);
                for claim in claims {
                    let status = if claim.is_stale { "STALE" } else { "active" };
                    println!(
                        "  {} (pid={}, hostname={}, age={}s) [{}]",
                        claim.work_id,
                        claim.pid,
                        claim.hostname,
                        Utc::now()
                            .signed_duration_since(claim.claimed_at)
                            .num_seconds(),
                        status
                    );
                }
            }
        } else {
            // List claims for all profiles
            let mut all_claims = Vec::new();
            for (profile_name, entries) in &state.claims {
                for entry in entries {
                    let detail = match entry {
                        WorkClaimStateEntry::V1(id) => ClaimDetail {
                            work_id: id.clone(),
                            pid: 0,
                            hostname: "unknown".to_string(),
                            claimed_at: Utc::now(),
                            is_stale: true,
                        },
                        WorkClaimStateEntry::V2(claim) => {
                            let is_stale = claim.pid == 0 || !is_process_alive(claim.pid);
                            ClaimDetail {
                                work_id: claim.work_id.clone(),
                                pid: claim.pid,
                                hostname: claim.hostname.clone(),
                                claimed_at: claim.claimed_at,
                                is_stale,
                            }
                        }
                    };
                    all_claims.push((profile_name.clone(), detail));
                }
            }

            if json {
                #[derive(Serialize)]
                struct ProfileClaim {
                    profile: String,
                    #[serde(flatten)]
                    claim: ClaimDetail,
                }
                let json_claims: Vec<ProfileClaim> = all_claims
                    .into_iter()
                    .map(|(profile, claim)| ProfileClaim { profile, claim })
                    .collect();
                println!("{}", serde_json::to_string(&json_claims)?);
            } else if all_claims.is_empty() {
                println!("No claims across all profiles");
            } else {
                println!("All claims:");
                for (profile, claim) in all_claims {
                    let status = if claim.is_stale { "STALE" } else { "active" };
                    println!(
                        "  {}: {} (pid={}, hostname={}, age={}s) [{}]",
                        profile,
                        claim.work_id,
                        claim.pid,
                        claim.hostname,
                        Utc::now()
                            .signed_duration_since(claim.claimed_at)
                            .num_seconds(),
                        status
                    );
                }
            }
        }

        Ok(())
    })
}

/// Handle `gah claims clear` command
pub fn handle_claims_clear(profile: &str, work_id: &str) -> Result<()> {
    with_locked_state(|state| {
        state.ensure_v2();
        state.release(profile, work_id);
        Ok(())
    })
}

/// Handle `gah claims reclaim` command
pub fn handle_claims_reclaim(profile: &str, max_age_secs: u64) -> Result<Vec<String>> {
    with_locked_state(|state| {
        state.ensure_v2();
        Ok(state.reclaim_stale_claims(profile, max_age_secs))
    })
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

    #[test]
    fn test_v1_migration_and_stale_reclamation() {
        let mut state = WorkClaimState::new();
        state.version = 1;
        state.claims = std::collections::HashMap::new();
        state.claims.insert(
            "test_profile".to_string(),
            vec![
                WorkClaimStateEntry::V1("work_1".to_string()),
                WorkClaimStateEntry::V1("work_2".to_string()),
            ],
        );

        // Test migration
        state.ensure_v2();
        assert_eq!(state.version, 2);

        // Test that v1 claims are marked as stale
        assert!(state.is_claim_stale("test_profile", "work_1", 3600));
        assert!(state.is_claim_stale("test_profile", "work_2", 3600));

        // Test reclamation
        let reclaimed = state.reclaim_stale_claims("test_profile", 3600);
        assert_eq!(reclaimed.len(), 2);
        assert!(reclaimed.contains(&"work_1".to_string()));
        assert!(reclaimed.contains(&"work_2".to_string()));

        // Verify claims are removed
        assert!(!state.is_claimed("test_profile", "work_1"));
        assert!(!state.is_claimed("test_profile", "work_2"));
    }

    #[test]
    fn test_v2_claim_lifecycle() {
        let mut state = WorkClaimState::new();

        // Test claiming
        state.claim("test_profile", "work_1");
        assert!(state.is_claimed("test_profile", "work_1"));

        let claims = state.get_claimed("test_profile");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0], "work_1");

        // Test that the claim has proper metadata
        let details = state.get_claims_with_details("test_profile");
        assert_eq!(details.len(), 1);
        let claim = &details[0];
        assert_eq!(claim.work_id, "work_1");
        assert!(claim.pid > 0); // Should have a valid PID
        assert!(!claim.hostname.is_empty()); // Should have a hostname

        // Test releasing
        state.release("test_profile", "work_1");
        assert!(!state.is_claimed("test_profile", "work_1"));
    }

    #[test]
    fn test_stale_claim_detection() {
        let mut state = WorkClaimState::new();

        // Create a claim with PID 0 (simulating a dead process)
        state.claims.insert(
            "test_profile".to_string(),
            vec![WorkClaimStateEntry::V2(WorkClaim {
                work_id: "work_1".to_string(),
                pid: 0, // Dead process
                hostname: "test-host".to_string(),
                claimed_at: Utc::now(),
            })],
        );

        // Should be detected as stale
        assert!(state.is_claim_stale("test_profile", "work_1", 3600));

        // Create a claim with a very old timestamp
        let old_time = Utc::now() - chrono::Duration::hours(2);
        state.claims.insert(
            "test_profile2".to_string(),
            vec![WorkClaimStateEntry::V2(WorkClaim {
                work_id: "work_2".to_string(),
                pid: 12345, // Alive process but old claim
                hostname: "test-host".to_string(),
                claimed_at: old_time,
            })],
        );

        // Should be detected as stale due to age
        assert!(state.is_claim_stale("test_profile2", "work_2", 3600));
    }
}
