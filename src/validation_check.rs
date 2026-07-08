//! TICKET-073: durable self-verification state for each profile's
//! `validation_commands`.
//!
//! `gah dispatch`/`gah loop` runs a profile's `validation_commands` against a
//! pristine worktree on every dispatch. A broken entry can pass every manual
//! test in an already-built checkout yet still be broken in a genuinely fresh
//! `git worktree add` -- the actual environment every real dispatch runs in.
//! Such a broken entry only surfaces when a *real* dispatch hits it, wasting a
//! backend run and producing misleading failures that look like the *ticket's*
//! fault rather than the *gate's*.
//!
//! This module lets the harness verify the gate itself *before* trusting it:
//! the commands are hashed (order-sensitive, simple FNV-1a -- no crypto
//! needed). If the stored hash matches the current config *and* the last check
//! passed, the self-check is skipped entirely (fast path, ~zero cost). If it
//! differs (or nothing is stored yet, or the last check failed), the harness
//! spins up a fresh worktree, runs the commands once, records pass/fail + the
//! new hash, and cleans up.
//!
//! State is **per-profile** (each profile has its own `validation_commands`),
//! unlike `availability.rs` which is global. It lives under the same XDG
//! location pattern -- `$XDG_STATE_HOME/gah/validation_check.json` (falling
//! back to `~/.local/state/gah/validation_check.json`) -- and uses the same
//! atomic write-temp-then-rename + exclusive-lock pattern, so concurrent GAH
//! processes can't corrupt it.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub const CURRENT_VERSION: u32 = 1;

/// Per-profile self-check record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileValidationCheck {
    /// Order-sensitive hash of the profile's `validation_commands` list.
    pub commands_hash: String,
    /// RFC3339 timestamp of the last self-check run for this profile.
    pub last_verified_at: String,
    /// Whether the last self-check passed. A failed check is *not* treated as
    /// "verified ok" even if the hash matches -- it must be re-verified.
    #[serde(default)]
    pub last_verified_ok: bool,
}

/// Top-level state file: a keyed map of profile name -> its self-check record.
/// Keyed (not append-only like the ledger/availability) because each profile
/// has exactly one current gate state and the whole point is to detect when
/// *that* profile's commands change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCheckState {
    pub version: u32,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileValidationCheck>,
}

impl Default for ValidationCheckState {
    fn default() -> Self {
        ValidationCheckState {
            version: CURRENT_VERSION,
            profiles: BTreeMap::new(),
        }
    }
}

/// FNV-1a 64-bit hash over the (order-sensitive) command list. A simple, fast,
/// non-cryptographic string hash -- exactly what the ticket asks for. A
/// separator byte between entries ensures `["a", "b"]` hashes differently from
/// `["ab"]`, and an empty list hashes to a stable deterministic value.
pub fn hash_validation_commands(commands: &[String]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = OFFSET;
    for cmd in commands {
        for b in cmd.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(PRIME);
        }
        // Field separator so concatenation of two entries never collides with
        // a single longer entry.
        hash ^= 0x1f;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{:016x}", hash)
}

/// Pure decision: given the loaded state, a profile name, and the current
/// config hash, should the harness re-run the gate against a fresh worktree?
///
/// Returns `false` (skip, fast path) only when an entry exists for the profile
/// whose hash matches *and* whose last check passed. Any other situation --
/// missing entry, hash mismatch, or a previously-failed check -- returns
/// `true` (re-verify now).
pub fn should_recheck(state: &ValidationCheckState, profile: &str, hash: &str) -> bool {
    !matches!(
        state.profiles.get(profile),
        Some(entry) if entry.commands_hash == hash && entry.last_verified_ok
    )
}

fn resolve_state_path_from_env(xdg_state_home: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(xdg) = xdg_state_home.filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("gah").join("validation_check.json");
    }
    let home = home.unwrap_or("/root");
    PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("gah")
        .join("validation_check.json")
}

/// Resolve the state file path from the real environment. `GAH_VALIDATION_CHECK_PATH`
/// is an explicit override, matching the existing `GAH_AVAILABILITY_PATH` /
/// `GAH_LEDGER_PATH` convention so tests and CI can redirect it.
pub fn resolve_state_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_VALIDATION_CHECK_PATH") {
        return PathBuf::from(path);
    }
    resolve_state_path_from_env(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

fn lock_path_for(state_path: &Path) -> PathBuf {
    let mut lock_name = state_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "validation_check.json".into());
    lock_name.push(".lock");
    state_path.with_file_name(lock_name)
}

/// Read the state file, if present. A missing file is `Ok(default)` (no state
/// recorded yet = nothing verified, so the caller must re-check). A
/// present-but-malformed file or an unsupported version is an actionable `Err`,
/// never silently treated as empty.
pub fn load_state(state_path: &Path) -> Result<ValidationCheckState> {
    let text = match fs::read_to_string(state_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ValidationCheckState::default())
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", state_path.display())),
    };
    let state: ValidationCheckState = serde_json::from_str(&text)
        .with_context(|| format!("parsing validation-check state {}", state_path.display()))?;
    if state.version != CURRENT_VERSION {
        anyhow::bail!(
            "validation-check state {} has unsupported schema version {} (expected {}); refusing to read or overwrite it",
            state_path.display(),
            state.version,
            CURRENT_VERSION,
        );
    }
    Ok(state)
}

/// Record (upsert) a profile's self-check result under an exclusive advisory
/// lock, using an atomic write-temp-then-rename so readers never observe a
/// partial file and a crash mid-write can never corrupt the previous good
/// state.
pub fn record_check(
    state_path: &Path,
    profile: &str,
    commands_hash: &str,
    ok: bool,
    verified_at: &str,
) -> Result<()> {
    let dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("creating directory {}", dir.display()))?;

    let lock_path = lock_path_for(state_path);
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening lock file {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking {}", lock_path.display()))?;

    let mut state = load_state(state_path)?;
    state.profiles.insert(
        profile.to_string(),
        ProfileValidationCheck {
            commands_hash: commands_hash.to_string(),
            last_verified_at: verified_at.to_string(),
            last_verified_ok: ok,
        },
    );

    let json =
        serde_json::to_string_pretty(&state).context("serializing validation-check state")?;
    let tmp_path = dir.join(format!(
        "{}.tmp.{}",
        state_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("validation_check.json"),
        std::process::id()
    ));
    {
        let mut tmp = File::create(&tmp_path)
            .with_context(|| format!("creating temp file {}", tmp_path.display()))?;
        tmp.write_all(json.as_bytes())
            .with_context(|| format!("writing temp file {}", tmp_path.display()))?;
        tmp.sync_all().ok();
    }
    fs::rename(&tmp_path, state_path).with_context(|| {
        format!(
            "renaming {} to {}",
            tmp_path.display(),
            state_path.display()
        )
    })?;

    FileExt::unlock(&lock_file).ok();
    Ok(())
}

/// Format the current time as RFC3339, for `last_verified_at`.
pub fn now_rfc3339(now: OffsetDateTime) -> String {
    now.format(&Rfc3339)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn path(tmp: &TempDir) -> PathBuf {
        tmp.path().join("validation_check.json")
    }

    // ── hash_validation_commands ─────────────────────────────────────────

    #[test]
    fn hash_is_order_sensitive() {
        let a = vec!["cargo test".to_string(), "cargo fmt".to_string()];
        let b = vec!["cargo fmt".to_string(), "cargo test".to_string()];
        assert_ne!(
            hash_validation_commands(&a),
            hash_validation_commands(&b),
            "order changes must change the hash"
        );
    }

    #[test]
    fn hash_separates_entries_so_concatenation_cannot_collide() {
        let split = vec!["a".to_string(), "b".to_string()];
        let joined = vec!["ab".to_string()];
        assert_ne!(
            hash_validation_commands(&split),
            hash_validation_commands(&joined),
            "['a','b'] must not hash like ['ab']"
        );
    }

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let cmds = vec!["npm ci".to_string(), "npx tsc -b".to_string()];
        assert_eq!(
            hash_validation_commands(&cmds),
            hash_validation_commands(&cmds),
        );
    }

    #[test]
    fn empty_commands_hash_to_a_stable_value() {
        let h = hash_validation_commands(&[]);
        assert_eq!(h, hash_validation_commands(&[]));
        assert!(!h.is_empty());
    }

    // ── should_recheck ───────────────────────────────────────────────────

    #[test]
    fn skips_when_hash_matches_and_last_check_ok() {
        let mut state = ValidationCheckState::default();
        state.profiles.insert(
            "gah".to_string(),
            ProfileValidationCheck {
                commands_hash: "deadbeef".to_string(),
                last_verified_at: "2026-01-01T00:00:00Z".to_string(),
                last_verified_ok: true,
            },
        );
        assert!(!should_recheck(&state, "gah", "deadbeef"));
    }

    #[test]
    fn rechecks_when_hash_differs() {
        let mut state = ValidationCheckState::default();
        state.profiles.insert(
            "gah".to_string(),
            ProfileValidationCheck {
                commands_hash: "oldhash".to_string(),
                last_verified_at: "2026-01-01T00:00:00Z".to_string(),
                last_verified_ok: true,
            },
        );
        assert!(should_recheck(&state, "gah", "newhash"));
    }

    #[test]
    fn rechecks_when_last_check_failed_even_if_hash_matches() {
        let mut state = ValidationCheckState::default();
        state.profiles.insert(
            "gah".to_string(),
            ProfileValidationCheck {
                commands_hash: "same".to_string(),
                last_verified_at: "2026-01-01T00:00:00Z".to_string(),
                last_verified_ok: false,
            },
        );
        assert!(should_recheck(&state, "gah", "same"));
    }

    #[test]
    fn rechecks_when_no_entry_exists() {
        let state = ValidationCheckState::default();
        assert!(should_recheck(&state, "gah", "anything"));
    }

    // ── record_check / load_state round-trip ─────────────────────────────

    #[test]
    fn record_then_load_round_trips_per_profile() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);

        record_check(&p, "gah", "hash1", true, "2026-01-01T00:00:00Z").unwrap();
        record_check(&p, "other", "hash2", false, "2026-01-02T00:00:00Z").unwrap();

        let state = load_state(&p).unwrap();
        assert_eq!(state.profiles.len(), 2);

        let gah = state.profiles.get("gah").unwrap();
        assert_eq!(gah.commands_hash, "hash1");
        assert!(gah.last_verified_ok);

        let other = state.profiles.get("other").unwrap();
        assert_eq!(other.commands_hash, "hash2");
        assert!(!other.last_verified_ok);
    }

    #[test]
    fn record_upserts_existing_profile() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);

        record_check(&p, "gah", "hash1", true, "2026-01-01T00:00:00Z").unwrap();
        record_check(&p, "gah", "hash2", false, "2026-01-02T00:00:00Z").unwrap();

        let state = load_state(&p).unwrap();
        assert_eq!(state.profiles.len(), 1);
        let gah = state.profiles.get("gah").unwrap();
        assert_eq!(gah.commands_hash, "hash2");
        assert!(!gah.last_verified_ok);
    }

    #[test]
    fn load_defaults_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("does-not-exist.json");
        let state = load_state(&p).unwrap();
        assert_eq!(state.version, CURRENT_VERSION);
        assert!(state.profiles.is_empty());
    }

    #[test]
    fn atomic_write_leaves_valid_json_and_no_temp_leftover() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        record_check(&p, "gah", "h", true, "2026-01-01T00:00:00Z").unwrap();

        let text = fs::read_to_string(&p).unwrap();
        let _: ValidationCheckState = serde_json::from_str(&text).unwrap();

        let leftover: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftover.is_empty());
    }

    // ── path resolution ──────────────────────────────────────────────────

    #[test]
    fn xdg_state_home_path_resolution() {
        let resolved = resolve_state_path_from_env(Some("/custom/xdg-state"), Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/custom/xdg-state/gah/validation_check.json")
        );
    }

    #[test]
    fn fallback_local_state_path_resolution() {
        let resolved = resolve_state_path_from_env(None, Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.local/state/gah/validation_check.json")
        );
    }

    #[test]
    fn empty_xdg_state_home_falls_back_like_unset() {
        let resolved = resolve_state_path_from_env(Some(""), Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.local/state/gah/validation_check.json")
        );
    }
}
