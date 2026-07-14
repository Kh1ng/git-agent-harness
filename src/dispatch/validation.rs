use super::command::command_output;
use crate::config::{GahConfig, Profile};
use crate::validation_runner::validate;
use crate::worktree;
use anyhow::{Context, Result};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use time::OffsetDateTime;

/// Marks an error as a failed validation-gate self-check rather than a
/// ticket, backend, or transient controller failure. The controller uses this
/// typed boundary to pause a loop instead of repeatedly retrying a gate that
/// has already proved broken.
#[derive(Debug)]
pub struct ValidationGateError;

impl fmt::Display for ValidationGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("validation gate self-check failed")
    }
}

impl std::error::Error for ValidationGateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ValidationFailureProgress {
    Changed,
    UnchangedFromBaseline,
    UnchangedFromPreviousAttempt,
    UnchangedFromBaselineAndPreviousAttempt,
}

impl ValidationFailureProgress {
    pub(super) fn unchanged_from_baseline(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromBaseline | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }

    pub(super) fn unchanged_from_previous_attempt(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromPreviousAttempt | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }
}

pub(super) fn validation_failure_no_progress_reason(
    progress: ValidationFailureProgress,
) -> Option<&'static str> {
    match progress {
        ValidationFailureProgress::Changed => None,
        ValidationFailureProgress::UnchangedFromBaseline => Some(
            "validation failure identical to the pristine-tree baseline — the agent's changes never affected this error. Fix the validation command or environment, not the ticket.",
        ),
        ValidationFailureProgress::UnchangedFromPreviousAttempt => Some(
            "validation failure identical to the previous attempt — the agent made no progress on the failing check.",
        ),
        ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt => Some(
            "validation failure identical to both the pristine-tree baseline and the previous attempt — the agent made no progress and never affected the original error.",
        ),
    }
}

/// Run `auto_fix_commands` in the worktree, best-effort, right before
/// `validate()`. A formatter failing to run (missing binary, whatever) must
/// never block the dispatch -- it's a convenience, not a gate -- so every
/// failure is logged and swallowed rather than propagated.
pub(super) fn run_auto_fix_commands(commands: &[String], wt: &Path, env_vars: &[(String, String)]) {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        let mut command = Command::new("sh");
        command.args(["-c", cmd_str]).current_dir(wt);
        for (key, value) in env_vars {
            command.env(key, value);
        }
        match command.output() {
            Ok(out) if !out.status.success() => {
                eprintln!(
                    "warning: auto_fix command '{cmd_str}' exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Err(e) => {
                eprintln!("warning: auto_fix command '{cmd_str}' failed to run: {e:#}");
            }
            Ok(_) => {}
        }
    }
}

pub(super) fn validation_env(profile: &Profile, session_scope: &Path) -> Vec<(String, String)> {
    vec![(
        "CARGO_TARGET_DIR".to_string(),
        crate::build_cache::target_dir(&profile.artifact_root, session_scope)
            .to_string_lossy()
            .into_owned(),
    )]
}

/// TICKET-073: verify a profile's `validation_commands` against a genuinely
/// fresh worktree before trusting the dispatch gate.
///
/// This is the "verify the gate itself works before trusting it" check. The
/// common case (commands unchanged since the last successful self-check) is a
/// pure hash compare against durable state and costs essentially nothing. Only
/// on a hash change (or no prior record, or a previously-failed check) does
/// this spin up a real isolated worktree from `default_target_branch`, run the
/// commands once, record pass/fail + the new hash, and clean the worktree up.
///
/// On success this returns `Ok(())` and may have written a new state record.
/// On a failed self-check it bails with a distinct, loud error — the failure
/// class is `FailureClass::ValidationGate`, deliberately *not*
/// `FailureClass::ValidationFailure`, so a broken config is never conflated
/// with the dispatched ticket's own outcome.
///
/// `skip` honours an explicit operator bypass (passed through
/// `--skip-validation-gate`); everything else is fail-closed.
pub fn self_check_validation_gate(profile: &Profile, cfg: &GahConfig, skip: bool) -> Result<()> {
    use crate::validation_check as vc;

    if skip {
        println!("[validation-gate] skipped by explicit --skip-validation-gate");
        return Ok(());
    }

    // Nothing to verify when the profile has no validation commands at all.
    if profile.validation_commands.is_empty() {
        return Ok(());
    }

    let repo = Path::new(&profile.local_path);
    let target_sha = command_output("git", &["rev-parse", &profile.default_target_branch], repo)
        .map_err(|error| {
            // A dedicated ValidationGateError, not a generic error: this is
            // the gate itself failing to even run (e.g. default_target_branch
            // was renamed/deleted, or a shallow clone is missing the ref),
            // not a transient dispatch hiccup. Left as a plain error, it
            // would be misclassified by is_validation_gate_failure and the
            // daemon would retry it every 5 minutes forever instead of
            // pausing with a clear, actionable message like every other
            // broken-gate state in this function.
            anyhow::Error::new(ValidationGateError).context(format!(
                "VALIDATION GATE FAILED — could not resolve target branch '{}' for profile \
                 '{}': {error:#}",
                profile.default_target_branch, profile.repo_id
            ))
        })?;
    let gate_environment_signature =
        crate::build_cache::validation_environment_signature(&profile.artifact_root);
    let hash = vc::hash_validation_context(
        &profile.validation_commands,
        target_sha.trim(),
        &gate_environment_signature,
    );
    let state_path = vc::resolve_state_path();

    // Hold a per-profile lock across the whole decide-then-verify sequence,
    // not just the final write. Same-profile callers then share one proof,
    // while validation commands that invoke GAH for a different test profile
    // cannot deadlock behind their parent. The state file's global lock is
    // taken only for the short atomic record update below.
    let profile_lock = vc::acquire_profile_lock(&state_path, &profile.repo_id)?;
    if crate::runner::shutdown_requested() {
        fs2::FileExt::unlock(&profile_lock).ok();
        anyhow::bail!("shutdown requested before validation gate self-check");
    }

    let state = vc::load_state(&state_path)
        .with_context(|| format!("loading validation-check state {}", state_path.display()))?;

    if !vc::should_recheck(&state, &profile.repo_id, &hash) {
        println!(
            "[validation-gate] commands unchanged (hash {}) — skipping fresh-worktree self-check",
            &hash[..hash.len().min(8)]
        );
        fs2::FileExt::unlock(&profile_lock).ok();
        return Ok(());
    }

    println!(
        "[validation-gate] commands changed (hash {}) — verifying against a fresh worktree from '{}'...",
        &hash[..hash.len().min(8)],
        profile.default_target_branch
    );

    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);

    // `worktree::create` errors out if the branch already exists. Use a
    // full-precision timestamp + random suffix so the branch name is truly
    // unique per run — the previous code truncated to 8 alphanumeric chars
    // from RFC3339, which collapsed to just the date (`20260709`) and
    // caused every same-day gate run after the first to fail.
    let ts = vc::now_rfc3339(OffsetDateTime::now_utc());
    let ts_compact: String = ts.chars().filter(|c| c.is_alphanumeric()).collect();
    let suffix = &ts_compact[..ts_compact.len().min(20)];
    let branch = format!(
        "gah/validation-gate-{}-{}",
        &hash[..hash.len().min(8)],
        suffix
    );

    let wt = worktree::create(
        repo,
        &profile.default_target_branch,
        &branch,
        &worktree_base,
    )?;
    let timeout = std::time::Duration::from_secs(profile.review_timeout_seconds());
    let cargo_target = crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, &wt)?;
    let gate_environment = cargo_target.environment();
    let verified_at = vc::now_rfc3339(OffsetDateTime::now_utc());
    let result = validate(
        &profile.validation_commands,
        &wt,
        &gate_environment,
        timeout,
    );
    let ok = result.is_ok();

    // Always clean up, regardless of pass/fail — a leftover validation-gate
    // worktree AND branch is state noise that the next dispatch would trip
    // over. The branch must be deleted too: worktree::cleanup only removes
    // the worktree dir and prunes, leaving the branch ref behind.
    worktree::cleanup(&wt, repo);
    let _ = worktree::git_raw(&["branch", "-D", &branch], repo);

    if crate::runner::shutdown_requested() {
        fs2::FileExt::unlock(&profile_lock).ok();
        anyhow::bail!("shutdown requested during validation gate self-check");
    }

    let record_result = vc::record_check(&state_path, &profile.repo_id, &hash, ok, &verified_at)
        .with_context(|| format!("recording validation-check result {}", state_path.display()));
    fs2::FileExt::unlock(&profile_lock).ok();
    record_result?;

    if let Err(text) = result {
        return Err(anyhow::Error::new(ValidationGateError).context(format!(
            "VALIDATION GATE FAILED — profile '{}' validation_commands did not pass on a \
             fresh worktree from '{}'. This is a broken gate config, NOT the dispatched \
             ticket's fault. Fix validation_commands (or run with --skip-validation-gate to \
             proceed anyway once you've acknowledged it).\n\n\
             Self-check recorded last_verified_ok=false (hash {}).\n\n\
             Failure output:\n{}",
            profile.repo_id,
            profile.default_target_branch,
            &hash[..hash.len().min(8)],
            text,
        )));
    }

    println!(
        "[validation-gate] passed on fresh worktree — self-check recorded (hash {})",
        &hash[..hash.len().min(8)]
    );
    Ok(())
}

/// Whether `improve()` can skip its own per-dispatch pristine-worktree
/// baseline validation, relying instead on the profile-level validation gate
/// (`self_check_validation_gate`). That shared gate only ever proves
/// `profile.default_target_branch` -- so its proof only covers a dispatch
/// that skipping requires: a FRESH worktree cut from that branch (no
/// `existing_branch`, i.e. not a `FixMr`/repair dispatch, which validates the
/// existing MR branch instead) AND the gate not having been explicitly
/// bypassed (no shared proof exists in that case either).
pub(super) fn should_skip_per_dispatch_baseline(
    validation_commands_empty: bool,
    has_existing_branch: bool,
    skip_validation_gate: bool,
) -> bool {
    validation_commands_empty || (!has_existing_branch && !skip_validation_gate)
}

/// Extracts a stable fingerprint from raw validation failure output (combined
/// stdout+stderr from `validate()`) for `classify_validation_failure_progress`
/// to compare instead of the raw text.
///
/// Two attempts that hit the exact same mistake can still differ byte-for-byte:
/// clippy/rustc line:column numbers shift as surrounding code the agent wrote
/// changes shape, and the `Checking ... (path)` header embeds a worktree path
/// that can differ between dispatches. Comparing raw text would then miss a
/// genuine repeat and burn a whole extra attempt on a mistake that was never
/// going to resolve (observed live: TICKET-154's `dead_code` lint firing on
/// the same unwired functions across attempts).
///
/// Keeps only the diagnostic header lines (`error: ...`, `error[E...]: ...`,
/// `warning: ...`) that name the actual mistake, dropping `--> file:line:col`
/// locations, source snippets, and `= note:`/`= help:` lines that vary without
/// the mistake itself changing. Falls back to the full trimmed text when
/// nothing matches those markers (e.g. a cargo test panic/assertion failure),
/// so two dissimilar failures are never conflated into an identical empty
/// fingerprint.
pub(super) fn validation_failure_fingerprint(text: &str) -> String {
    let diagnostic_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("error") || line.starts_with("warning:"))
        .collect();
    if diagnostic_lines.is_empty() {
        text.trim().to_string()
    } else {
        diagnostic_lines.join("\n")
    }
}

pub(super) fn classify_validation_failure_progress(
    baseline_failure: Option<&str>,
    previous_failure: Option<&str>,
    current_failure: &str,
) -> ValidationFailureProgress {
    let current_fp = validation_failure_fingerprint(current_failure);
    let same_as_baseline = baseline_failure
        .map(validation_failure_fingerprint)
        .as_deref()
        == Some(current_fp.as_str());
    let same_as_previous = previous_failure
        .map(validation_failure_fingerprint)
        .as_deref()
        == Some(current_fp.as_str());
    match (same_as_baseline, same_as_previous) {
        (true, true) => ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt,
        (true, false) => ValidationFailureProgress::UnchangedFromBaseline,
        (false, true) => ValidationFailureProgress::UnchangedFromPreviousAttempt,
        (false, false) => ValidationFailureProgress::Changed,
    }
}

#[cfg(test)]
mod tests;
