use crate::config::Profile;
use anyhow::Result;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) mod backends;
pub(crate) mod output;
pub(crate) mod process;
#[allow(unused_imports)]
pub use crate::runner::backends::claude::{run_claude, run_claude_with_executable};
pub(crate) use crate::runner::backends::codex::{codex_model_args, filtered_codex_args};
#[allow(unused_imports)]
pub use crate::runner::backends::codex::{
    extract_model_from_args, extract_model_from_backend_args, filtered_backend_args, run_codex,
    run_codex_with_executable,
};
#[allow(unused_imports)]
pub use crate::runner::backends::opencode::{run_opencode, run_opencode_with_executable};
#[allow(unused_imports)]
pub use crate::runner::backends::openhands::{list_oh_profiles, load_oh_profile, run_openhands};
#[allow(unused_imports)]
pub use crate::runner::backends::vibe::{run_vibe, run_vibe_with_executable};

pub(crate) use crate::runner::process::copy_stream_to_file;
pub use crate::runner::process::install_shutdown_handler;
pub(crate) use crate::runner::process::kill_process_group;
pub(crate) use crate::runner::process::prepare_process_group;
pub use crate::runner::process::shutdown_requested;
pub(crate) use crate::runner::process::spawn_with_idle_watch;
#[allow(unused_imports)]
pub(crate) use crate::runner::process::spawn_with_worktree_progress_watch;
pub(crate) use crate::runner::process::write_redacted_task;

/// Parse a KEY=VALUE env file, skipping blank lines and comments.
pub fn load_env_file(path: &str) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            ))
        })
        .collect()
}

#[derive(Debug)]
pub struct RunResult {
    pub exit_code: i32,
    pub duration_secs: f64,
    pub log_path: String,
    /// Authoritative final assistant text selected by the backend adapter.
    /// Raw stdout/stderr is never promoted into this field.
    pub final_summary: Option<String>,
    /// TICKET-066/#155: for AGY backends, the bytes appended to AGY's
    /// `cli.log` during this specific run, scoped to the pre-run byte
    /// offset (so concurrent appends from other AGY instances/log sources
    /// are excluded). `None` for non-AGY backends and for runs where the
    /// cli.log could not be read. This delta — not a fresh read of the
    /// whole log — is what usage/quota parsing consumes, so a single
    /// attempt's usage is never polluted by prior runs.
    pub agy_cli_log_delta: Option<String>,
    /// A run-scoped tail of a backend-owned diagnostic log. Unlike
    /// `log_path`, this is not CLI stdout/stderr: some backends (notably
    /// OpenCode) write provider failures only to their own internal log.
    /// The tail begins at the pre-run byte offset, so old failures and other
    /// runs cannot poison routing for this attempt.
    pub internal_log_delta: Option<String>,
    /// Source path for `internal_log_delta`, retained for availability
    /// diagnostics. Missing/unreadable logs leave both fields `None`.
    pub internal_log_path: Option<String>,
    /// Backend-owned structured usage artifact. Claude uses its transcript
    /// JSONL; Vibe uses its session `meta.json`. `None` when the backend did
    /// not produce a discoverable artifact.
    pub transcript_path: Option<String>,
    /// AGY CLI version for this run (e.g. "1.0.16"), captured via
    /// `agy --version`. `None` for non-AGY backends and for runs where version
    /// detection fails. Used for log-path resolution and upstream log-format
    /// drift detection (TICKET-242).
    pub agy_version: Option<String>,
}

pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableResolution {
    Found(PathBuf),
    MissingExplicitPath(PathBuf),
    MissingFromPath(String),
    UnknownBackend(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProcessOutcome {
    Success,
    ExecutableUnavailable,
    SpawnFailure,
    NonZeroExit(i32),
    SignalTermination(i32),
    Timeout,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub struct ReviewRunResult {
    pub outcome: ReviewProcessOutcome,
    pub duration_secs: f64,
    pub stdout: String,
    pub stderr: String,
}

/// Run Antigravity CLI non-interactively via `agy --print`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_agy(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    env_vars: &[(String, String)],
    executable_name: &str,
) -> Result<RunResult> {
    run_agy_with_executable(
        Path::new(executable_name),
        worktree,
        task,
        session_dir,
        llm,
        env_vars,
        None,
        120,
    )
}

/// AGY sometimes exits 0 with empty stdout on a provider-side failure
/// (quota exhaustion, expired auth) instead of a non-zero exit -- shared
/// by the worker path (run_agy_with_executable) and the review path
/// (run_review_backend), both of which treat that as a failure needing a
/// real diagnosis, not a silently-empty "success".
fn agy_empty_output_diagnosis(env_vars: &[(String, String)], executable: &Path) -> String {
    let agy_home = env_vars
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.as_str())
        .map(|h| h.to_string())
        .or_else(|| std::env::var("HOME").ok());
    let Some(home) = agy_home else {
        return "AGY produced no output (exit=0) and HOME is unset — cannot inspect cli.log."
            .to_string();
    };
    let agy_log = PathBuf::from(home).join(".gemini/antigravity-cli/cli.log");
    let Ok(contents) = fs::read_to_string(&agy_log) else {
        return format!(
            "AGY produced no output (exit=0).  Check {} for details.",
            agy_log.display(),
        );
    };
    if contents.contains("RESOURCE_EXHAUSTED") || contents.contains("429") {
        format!(
            "AGY quota exhausted (exit=0 empty output).  See {}.  Resets ~{}.",
            agy_log.display(),
            extract_reset_time(&contents).unwrap_or_else(|| "unknown".into()),
        )
    } else if contents.contains("not logged into Antigravity") || contents.contains("not logged in")
    {
        format!(
            "AGY not authenticated.  Run `{}` interactively to log in.",
            executable.display(),
        )
    } else {
        format!(
            "AGY produced no output (exit=0).  Check {} for details.",
            agy_log.display(),
        )
    }
}

/// Detect AGY CLI version by running `agy --version`. Returns `None` on any
/// failure (missing binary, non-zero exit, unparseable output). Cheap and used
/// for log-path selection and upstream log-format drift detection (TICKET-242).
fn detect_agy_version(
    executable: &Path,
    worktree: &Path,
    env_vars: &[(String, String)],
) -> Option<String> {
    let mut cmd = Command::new(executable);
    cmd.arg("--version").current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Accept "antigravity-cli version 1.0.16" or a bare "1.0.16" (an optional
    // leading `v`/`V` is tolerated). AGY versions are `MAJOR.MINOR.PATCH`, so
    // take the last whitespace-separated token that looks like a dotted-numeric
    // version.
    version_str
        .split_whitespace()
        .rev()
        .find(|tok| {
            let t = tok.strip_prefix(['v', 'V']).unwrap_or(tok);
            t.split('.').count() >= 2 && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(|s| s.strip_prefix(['v', 'V']).unwrap_or(s).to_string())
}

/// AGY cli.log layout, keyed by the first upstream version that introduced it
/// (TICKET-242). A future upstream log relocation is a new table row here, not
/// a code archaeology session in `agy_cli_log_path`.
///
/// Each entry is `(first_version, candidate_paths)`: `first_version` is the
/// semver-style lower bound (inclusive) at which the layout appeared, and
/// `candidate_paths` are resolved relative to `~/.gemini/antigravity-cli` (a
/// file is used directly; a directory is scanned for the newest `cli-*` file).
const AGY_LOG_PATHS: &[(&str, &[&str])] = &[
    // Earliest releases: a single `cli.log` file.
    ("0.0.0", &["cli.log"]),
    // v1.0.0+: logs are rotated under `log/` as `cli-*.log`.
    ("1.0.0", &["log"]),
];

/// Compare two dotted-numeric version strings (e.g. "1.0.16" vs "1.0.0").
/// Returns the [`std::cmp::Ordering`] of `a` relative to `b`. Non-numeric
/// components compare as `0`, which is fine for the `MAJOR.MINOR.PATCH`
/// versions AGY emits.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ap: Vec<u64> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let bp: Vec<u64> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    let max = ap.len().max(bp.len());
    for i in 0..max {
        let av = *ap.get(i).unwrap_or(&0);
        let bv = *bp.get(i).unwrap_or(&0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Candidate cli.log locations (relative to the AGY root) for the detected
/// version. When the version is unknown we consider every layout ever seen, so
/// resolution still works against an unrecognized/old CLI.
fn agy_log_candidates(version: Option<&str>) -> Vec<&'static str> {
    // AGY_LOG_PATHS is defined oldest-first, but candidates must be tried
    // newest-first: a version at or above multiple thresholds (the common
    // case, since thresholds are cumulative lower bounds) would otherwise
    // resolve the OLDEST matching layout first. A stale `cli.log` left over
    // from a pre-1.0.0 install, sitting alongside a freshly-populated `log/`
    // directory the upgraded CLI is actually writing to, would then win --
    // silently reading a dead log file for exactly the upgrade-transition
    // window this table exists to handle correctly.
    match version {
        Some(v) => AGY_LOG_PATHS
            .iter()
            .rev()
            .filter(|(first, _)| version_cmp(v, first) != std::cmp::Ordering::Less)
            .flat_map(|(_, cands)| cands.iter().copied())
            .collect(),
        None => AGY_LOG_PATHS
            .iter()
            .rev()
            .flat_map(|(_, cands)| cands.iter().copied())
            .collect(),
    }
}

/// Resolve AGY's cli.log path from the HOME that the run actually uses
/// (the per-call `env_vars` win over process `HOME`, matching how the run's
/// effective HOME is resolved elsewhere). `version` selects the candidate
/// layout(s) from `AGY_LOG_PATHS` (keyed by version range); when `None` every
/// known layout is tried. Returns `None` only when no HOME is discoverable or
/// no candidate log exists -- in which case there is no cli.log to delta
/// against.
fn agy_cli_log_path(
    env_vars: &[(String, String)],
    _executable: &Path,
    version: Option<&str>,
) -> Option<PathBuf> {
    let home = env_vars
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("HOME").ok())?;
    let root = PathBuf::from(home).join(".gemini/antigravity-cli");
    for rel in agy_log_candidates(version) {
        let cand = root.join(rel);
        if cand.is_file() {
            return Some(cand);
        }
        if cand.is_dir() {
            if let Some(newest) = newest_cli_in(&cand) {
                return Some(newest);
            }
        }
    }
    None
}

/// Newest `cli-*` file inside `dir`, used for rotated AGY log layouts.
fn newest_cli_in(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.file_name()?.to_string_lossy().starts_with("cli-") {
            continue;
        }
        let modified = fs::metadata(&path).ok()?.modified().ok()?;
        if newest
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

/// Returns `true` when `delta` is non-empty yet contains none of the known AGY
/// cli.log signatures. That is the TICKET-242 drift canary: an AGY run failed
/// with empty output, the current-run log delta matched zero known signatures,
/// and the delta is non-empty -- so upstream has silently changed its log
/// format/path and classification has degraded.
fn is_agy_log_format_unrecognized(delta: &str) -> bool {
    if delta.trim().is_empty() {
        return false;
    }
    const KNOWN_SIGNATURES: &[&str] = &[
        "RESOURCE_EXHAUSTED",
        "Individual quota reached",
        "Quota exceeded",
        "quota has been reached",
        "quota reached",
        "not logged into Antigravity",
        "not logged in",
        "AGY not authenticated",
        "Resets in ",
        "resets at ",
        "Your quota resets at",
    ];
    !KNOWN_SIGNATURES.iter().any(|sig| delta.contains(sig))
}

/// Read only bytes appended to a backend-owned log after `pre_offset`.
/// Returns `None` on missing/unreadable logs and treats a truncated/unchanged
/// log as no delta. Lossy decoding deliberately preserves diagnostic text
/// even if a backend left a partial UTF-8 write while exiting.
pub(crate) fn log_delta(log: &Option<PathBuf>, pre_offset: u64) -> Option<String> {
    let path = log.as_ref()?;
    let bytes = fs::read(path).ok()?;
    if (pre_offset as usize) >= bytes.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[pre_offset as usize..]).into_owned())
}

#[allow(clippy::too_many_arguments)]
pub fn run_agy_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    env_vars: &[(String, String)],
    print_timeout_seconds: Option<u64>,
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    // TICKET-242: detect the AGY CLI version up front. It is cheap, good
    // attribution, and drives log-path resolution plus upstream log-format
    // drift detection below.
    let agy_version = detect_agy_version(executable, worktree, env_vars);

    let mut cmd = Command::new(executable);
    cmd.arg("--print");
    cmd.arg(task);
    cmd.args(["--model", llm.model.as_str()]);
    if let Some(secs) = print_timeout_seconds {
        cmd.args(["--print-timeout", &format!("{secs}s")]);
    }

    // #155: scope AGY's cli.log to just the bytes this run appends. Capture
    // the pre-run byte offset up front so that, after the run, we read the
    // new tail only (not the whole log, which may contain unrelated
    // history or concurrent appends from other AGY instances). `None` when
    // the cli.log isn't readable yet is fine -- the delta stays `None` and
    // usage/quota parsing simply has nothing to draw from for this attempt.
    // The path is resolved via the version-keyed `AGY_LOG_PATHS` table.
    let agy_cli_log = agy_cli_log_path(env_vars, executable, agy_version.as_deref());
    let agy_cli_log_pre_offset = agy_cli_log
        .as_ref()
        .and_then(|p| fs::metadata(p).ok().map(|m| m.len()))
        .unwrap_or(0);
    cmd.arg("--dangerously-skip-permissions")
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    // GAH-side supervision: kill only when the log has genuinely gone quiet
    // for idle_timeout_seconds, not on a flat wall-clock budget. A model
    // that's slow but still producing output (still working) is never
    // killed for being slow; --print-timeout above stays as an outer
    // safety backstop for a truly hung process.
    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        &format!(
            "launching {}; is it installed and on PATH?",
            executable.display()
        ),
    )?;

    // Read captured stdout to detect silent failures. (A kill-for-idle
    // already leaves exit_code at -1, so the exit_code == 0 guard below
    // naturally skips this diagnosis for that case.)
    let output = fs::read_to_string(&log_path).unwrap_or_default();
    let trimmed = output.trim();

    // AGY sometimes exits 0 with empty output when quota is exhausted or
    // auth has expired.  Treat empty output at exit 0 as a failure and
    // try to classify the real cause from AGY's own log.
    if trimmed.is_empty() && exit_code == 0 {
        // TICKET-242 drift canary: if the run-scoped log delta is non-empty
        // but matches zero known signatures, upstream has silently changed its
        // log format/path. Emit a distinct note so the degradation is visible
        // the day it happens, instead of being silently classified unknown.
        let agy_cli_log_delta = log_delta(&agy_cli_log, agy_cli_log_pre_offset);
        if is_agy_log_format_unrecognized(agy_cli_log_delta.as_deref().unwrap_or("")) {
            let drift_msg = format!(
                "[agy_log_format_unrecognized] AGY produced no output with an unrecognized cli.log delta (agy_version={}). Upstream log format/path may have changed; quota/auth classification is degraded.",
                agy_version.as_deref().unwrap_or("unknown"),
            );
            if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
                let _ = writeln!(file, "{}", drift_msg);
            }
        }

        let err_msg = agy_empty_output_diagnosis(env_vars, executable);

        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
            let _ = writeln!(file, "{}", err_msg);
        }

        return Ok(RunResult {
            exit_code: -1,
            duration_secs,
            log_path: log_path.to_string_lossy().into_owned(),
            final_summary: None,
            agy_cli_log_delta,
            internal_log_delta: None,
            internal_log_path: None,
            transcript_path: None,
            agy_version: agy_version.clone(),
        });
    }

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        // AGY --print currently mixes stdout and stderr in the diagnostic
        // log and exposes no run-scoped structured conversation artifact.
        // Fail closed until its adapter can identify one authoritatively.
        final_summary: None,
        agy_cli_log_delta: log_delta(&agy_cli_log, agy_cli_log_pre_offset),
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version,
    })
}

/// Crude extraction of a reset timestamp from an AGY cli.log line such as
/// "Resets in 16m44s."  Returns `Some(..)` or `None` if no pattern found.
fn extract_reset_time(log: &str) -> Option<String> {
    for line in log.lines().rev() {
        if let Some(pos) = line.find("Resets in ") {
            let rest = &line[pos + 10..];
            if let Some(end) = rest.find(['.', ':']) {
                let reset = rest[..end].trim();
                if !reset.is_empty() {
                    return Some(reset.to_string());
                }
            }
            let end = rest.trim_end_matches('.');
            if !end.is_empty() {
                return Some(end.to_string());
            }
        }
    }
    None
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn backend_available(name: &str) -> bool {
    backend_command_name(name)
        .and_then(resolve_executable_on_path)
        .is_some()
}

pub fn backend_available_for_profile(profile: &Profile, name: &str) -> bool {
    matches!(
        resolve_backend_executable(profile, name),
        ExecutableResolution::Found(_)
    )
}

pub fn require_backend_executable(profile: &Profile, backend: &str) -> Result<PathBuf> {
    match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => Ok(path),
        ExecutableResolution::MissingExplicitPath(path) => {
            anyhow::bail!("configured executable '{}' does not exist", path.display())
        }
        ExecutableResolution::MissingFromPath(cmd) => {
            anyhow::bail!("required binary '{}' not found on PATH", cmd)
        }
        ExecutableResolution::UnknownBackend(backend) => {
            anyhow::bail!("unknown backend '{}'", backend)
        }
    }
}

pub fn resolve_backend_executable(profile: &Profile, backend: &str) -> ExecutableResolution {
    let Some(command) = backend_command_name(backend) else {
        return ExecutableResolution::UnknownBackend(backend.to_string());
    };
    if let Some(explicit) = profile.configured_backend_path(backend) {
        let path = PathBuf::from(explicit);
        return if is_executable_path(&path) {
            ExecutableResolution::Found(path)
        } else {
            ExecutableResolution::MissingExplicitPath(path)
        };
    }
    match resolve_executable_on_path(command) {
        Some(path) => ExecutableResolution::Found(path),
        None => ExecutableResolution::MissingFromPath(command.to_string()),
    }
}

pub fn run_review_backend(
    profile: &Profile,
    backend: &str,
    worktree: &Path,
    prompt: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    env_vars: &[(String, String)],
) -> ReviewRunResult {
    let start = Instant::now();
    let stdout_path = session_dir.join("review-stdout.log");
    let stderr_path = session_dir.join("review-stderr.log");
    let _ = write_redacted_task(session_dir, prompt);

    let executable = match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => path,
        ExecutableResolution::MissingExplicitPath(_) | ExecutableResolution::MissingFromPath(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::ExecutableUnavailable,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: String::new(),
            };
        }
        ExecutableResolution::UnknownBackend(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
            };
        }
    };

    if let Err(err) = fs::File::create(&stdout_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stdout_path.display()),
        };
    }
    if let Err(err) = fs::File::create(&stderr_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stderr_path.display()),
        };
    }

    let mut cmd = Command::new(&executable);
    match backend {
        "claude" => {
            cmd.args(["-p", prompt]).args(&profile.claude_args);
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
        }
        "codex" => {
            cmd.arg("exec")
                .arg(prompt)
                .args(filtered_codex_args(&profile.codex_args))
                .args(codex_model_args(effective_model));
        }
        "agy" | "agy-main" | "agy-second" => {
            cmd.arg("--print").arg(prompt);
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg("--dangerously-skip-permissions");
        }
        "vibe" => {
            cmd.arg("-p").arg(prompt);
            cmd.arg("--output").arg("text");
            cmd.arg("--trust");
            cmd.arg("--auto-approve");
            // Vibe does not accept --model flag; model selection is via
            // environment/config. We record the effective model separately
            // in ledger metadata but do not pass it to Vibe CLI.
        }
        "opencode" => {
            cmd.arg("run");
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg(prompt);
        }
        _ => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
            };
        }
    }
    cmd.current_dir(worktree)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_process_group(&mut cmd);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    if backend == "vibe" {
        if let Some(model) = effective_model {
            cmd.env("VIBE_ACTIVE_MODEL", model);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: err.to_string(),
            };
        }
    };
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| copy_stream_to_file(stdout, stdout_path.clone(), None));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, stderr_path.clone(), None));

    let timeout = Duration::from_secs(profile.review_timeout_seconds());
    let poll_interval = Duration::from_millis(25);
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    break ReviewProcessOutcome::Success;
                }
                if let Some(code) = status.code() {
                    break ReviewProcessOutcome::NonZeroExit(code);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(signal) = status.signal() {
                        break ReviewProcessOutcome::SignalTermination(signal);
                    }
                }
                break ReviewProcessOutcome::SpawnFailure;
            }
            Ok(None) => {
                if shutdown_requested() {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::SignalTermination(libc::SIGTERM);
                }
                if start.elapsed() >= timeout {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::Timeout;
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                kill_process_group(&mut child);
                let _ = child.wait();
                let mut stderr = read_text_file(&stderr_path);
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&err.to_string());
                return ReviewRunResult {
                    outcome: ReviewProcessOutcome::SpawnFailure,
                    duration_secs: start.elapsed().as_secs_f64(),
                    stdout: read_text_file(&stdout_path),
                    stderr,
                };
            }
        }
    };
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    let mut stdout = read_text_file(&stdout_path);
    // AGY sometimes exits 0 with empty stdout on a provider-side failure
    // (quota exhaustion, expired auth) -- the same silent-success failure
    // mode already handled for the worker path in run_agy_with_executable.
    // Left as Success here, parse_review_verdict would just fail with an
    // opaque "reviewer did not return verdict JSON" and never give the
    // caller (dispatch::review) a chance to recognize the real cause and
    // reroute to the next review_candidates entry -- so diagnose it the
    // same way the worker path does, and put the diagnosis in stdout
    // where mark_backend_unavailable_from_output can actually see it.
    let outcome = if matches!(backend, "agy" | "agy-main" | "agy-second")
        && matches!(outcome, ReviewProcessOutcome::Success)
        && stdout.trim().is_empty()
    {
        stdout = agy_empty_output_diagnosis(env_vars, &executable);
        ReviewProcessOutcome::NonZeroExit(-1)
    } else {
        outcome
    };

    ReviewRunResult {
        outcome,
        duration_secs: start.elapsed().as_secs_f64(),
        stdout,
        stderr: read_text_file(&stderr_path),
    }
}

fn backend_command_name(name: &str) -> Option<&'static str> {
    match name {
        "openhands" | "cloud-coder" | "auto" => Some("openhands"),
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "agy" => Some("agy"),
        "agy-main" => Some("agy-main"),
        "agy-second" => Some("agy-second"),
        "vibe" => Some("vibe"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

fn resolve_executable_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_path(candidate))
}

fn is_executable_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn read_text_file(path: &Path) -> String {
    let mut buf = Vec::new();
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use crate::test_support::PathGuard;
    use tempfile::TempDir;

    // ── run_agy ─────────────────────────────────────────────────────────

    #[test]
    fn run_agy_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_agy(
            &f.worktree,
            "agy task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-agy"));
        assert!(log.contains("stderr-marker-agy"));
    }

    #[test]
    fn run_agy_with_executable_passes_print_timeout_when_given() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "Gemini 3.5 Flash (Medium)".into(),
            },
            &envs,
            Some(900),
            120,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        let flag_pos = argv.iter().position(|a| a == "--print-timeout").unwrap();
        assert_eq!(argv[flag_pos + 1], "900s");
    }

    #[test]
    fn run_agy_with_executable_omits_print_timeout_when_absent() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "Gemini 3.5 Flash (Medium)".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(!argv.iter().any(|a| a == "--print-timeout"));
    }

    #[test]
    fn run_agy_with_executable_kills_process_after_idle_timeout_with_no_new_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Writes one line, then goes silent for far longer than the idle
        // timeout below -- this must be killed, not allowed to keep running.
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        // Needs the real `sleep` binary reachable, not just the fake bin_dir.
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &envs,
            None,
            1, // idle timeout: 1s of silence is stalled
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_agy_with_executable_does_not_kill_while_output_keeps_arriving() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Writes output every ~1s for 3s total -- longer than the 2s idle
        // timeout below, but never actually goes quiet for that long. Must
        // be allowed to finish naturally, not killed for being slow overall.
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\nfor i in 1 2 3; do echo \"step$i\"; sleep 1; done\n",
        );
        // Needs the real `sleep` binary reachable, not just the fake bin_dir.
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &envs,
            None,
            2, // idle timeout: longer than any single gap between writes
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1") && log.contains("step2") && log.contains("step3"));
        assert!(!log.contains("killed after"));
    }

    #[test]
    fn run_agy_core_argv_and_model_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy(
            &f.worktree,
            "the agy task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(argv.contains(&"the agy task".to_string()));
    }

    #[test]
    fn run_agy_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_agy(
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap_err();

        assert!(err.to_string().contains("launching agy; is it installed"));
    }

    #[test]
    fn resolve_backend_executable_prefers_explicit_path() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.claude_path = Some(f.bin_dir.join("claude-explicit").display().to_string());
        let _guard = PathGuard::set("");

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude-explicit"))
        );
    }

    #[test]
    fn resolve_backend_executable_falls_back_to_path_when_unset() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude", "#!/bin/sh\nexit 0\n");
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_invalid_explicit_path_is_unavailable() {
        let mut profile = test_profile();
        profile.claude_path = Some("/definitely/missing/claude".into());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::MissingExplicitPath(PathBuf::from("/definitely/missing/claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_supports_codex_and_agy_paths() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "codex-explicit", "#!/bin/sh\nexit 0\n");
        make_fake_bin(&f.bin_dir, "agy-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.codex_path = Some(f.bin_dir.join("codex-explicit").display().to_string());
        profile.agy_path = Some(f.bin_dir.join("agy-explicit").display().to_string());

        assert_eq!(
            resolve_backend_executable(&profile, "codex"),
            ExecutableResolution::Found(f.bin_dir.join("codex-explicit"))
        );
        assert_eq!(
            resolve_backend_executable(&profile, "agy"),
            ExecutableResolution::Found(f.bin_dir.join("agy-explicit"))
        );
    }

    #[test]
    fn run_review_backend_times_out_and_preserves_partial_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\necho 'partial review'\nsleep 2\necho 'late stderr' >&2\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Timeout);
        assert!(result.stdout.contains("partial review"));
    }

    #[test]
    fn run_review_backend_supports_agy_with_model_and_env() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "agy",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("Claude Sonnet 4.6 (Thinking)"),
            &[("FROM_ENV_FILE".into(), "agy-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-agy"));
        assert!(result.stderr.contains("stderr-marker-agy"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"Claude Sonnet 4.6 (Thinking)".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=agy-review-env"));
    }

    #[test]
    fn run_review_backend_supports_vibe_without_model_flag() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("mistral-medium-3.5"),
            &[("FROM_ENV_FILE".into(), "vibe-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-vibe"));
        assert!(result.stderr.contains("stderr-marker-vibe"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        // Vibe does NOT accept --model flag
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"mistral-medium-3.5".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-review-env"));
        assert!(env.contains("VIBE_ACTIVE_MODEL=mistral-medium-3.5"));
    }

    #[test]
    fn run_review_backend_binds_claude_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("haiku"),
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_review_backend_vibe_command_construction() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "Review this code: int main() { return 0; }",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);

        let argv = recorded_argv(&f.record_dir);
        // Verify correct command construction: vibe -p <prompt> --output text --trust --auto-approve
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        // Should NOT contain the invalid "review" subcommand
        assert!(!argv.contains(&"review".to_string()));
        // Should NOT contain --model flag
        assert!(!argv.contains(&"--model".to_string()));
    }

    // ── backend_available ────────────────────────────────────────────────
    // Not part of the spec's priority list, but it is a one-line pure-ish
    // wrapper around `which` that every routing decision depends on, and it
    // was previously completely untested.

    #[test]
    fn backend_available_false_for_unknown_backend_name() {
        assert!(!backend_available("not-a-real-backend"));
    }

    // ── AGY empty-output detection ────────────────────────────────────

    #[test]
    fn extract_reset_time_parses_standard_format() {
        let log = "RESOURCE_EXHAUSTED (code 429): quota. Resets in 16m44s.";
        assert_eq!(extract_reset_time(log).as_deref(), Some("16m44s"));
    }

    #[test]
    fn extract_reset_time_returns_none_when_absent() {
        assert_eq!(extract_reset_time("no reset info here"), None);
    }

    #[test]
    fn agy_empty_output_with_quota_log_detected_as_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Fake agy that exits 0 with no stdout/stderr.
        make_fake_bin(&f.bin_dir, "agy", "#!/bin/sh\nexit 0\n");
        // Write a cli.log with quota error text.
        let agy_home = f.record_dir.parent().unwrap();
        let agy_log_dir = agy_home.join(".gemini/antigravity-cli");
        fs::create_dir_all(&agy_log_dir).unwrap();
        fs::write(
            agy_log_dir.join("cli.log"),
            "RESOURCE_EXHAUSTED (code 429): quota. Resets in 10m.\n",
        )
        .unwrap();

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), agy_home.to_string_lossy().to_string()),
        ];
        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();
        // Empty output with quota error becomes exit_code=-1
        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("quota exhausted"), "got log: {log}");
    }

    #[test]
    fn agy_empty_output_with_auth_log_detected_as_auth_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "agy", "#!/bin/sh\nexit 0\n");
        let agy_home = f.record_dir.parent().unwrap();
        let agy_log_dir = agy_home.join(".gemini/antigravity-cli");
        fs::create_dir_all(&agy_log_dir).unwrap();
        fs::write(
            agy_log_dir.join("cli.log"),
            "error getting token source: You are not logged into Antigravity.\n",
        )
        .unwrap();

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), agy_home.to_string_lossy().to_string()),
        ];
        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();
        // Empty output with auth error becomes exit_code=-1
        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("not authenticated"), "got log: {log}");
    }

    #[test]
    fn agy_successful_output_not_affected_by_detection() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Normal agy that produces stdout content.
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_agy(
            &f.worktree,
            "normal task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-agy"), "normal output preserved");
    }

    // ── TICKET-242: AGY version + log-format drift detection ───────────────

    #[test]
    fn detect_agy_version_parses_version_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        make_fake_bin(
            tmp.path(),
            "agy",
            "#!/bin/sh\necho 'antigravity-cli version 1.0.16'\n",
        );
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]).as_deref(),
            Some("1.0.16")
        );
    }

    #[test]
    fn detect_agy_version_handles_bare_version_token() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        make_fake_bin(tmp.path(), "agy", "#!/bin/sh\necho 'v2.3.4'\n");
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]).as_deref(),
            Some("2.3.4")
        );
    }

    #[test]
    fn detect_agy_version_returns_none_on_failure() {
        let tmp = TempDir::new().unwrap();
        make_fake_bin(
            tmp.path(),
            "agy",
            "#!/bin/sh\necho 'not a version'\nexit 1\n",
        );
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]),
            None
        );
    }

    #[test]
    fn detect_agy_version_runs_inside_the_dispatch_worktree() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let observed_cwd = tmp.path().join("observed-cwd");
        make_fake_bin(
            tmp.path(),
            "agy",
            &format!(
                "#!/bin/sh\npwd > '{}'\necho 'antigravity-cli version 1.0.16'\n",
                observed_cwd.display()
            ),
        );

        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), &worktree, &[]).as_deref(),
            Some("1.0.16")
        );
        assert_eq!(
            fs::read_to_string(observed_cwd).unwrap().trim(),
            worktree.display().to_string()
        );
    }

    #[test]
    fn is_agy_log_format_unrecognized_classifies_correctly() {
        // Empty delta is never "unrecognized".
        assert!(!is_agy_log_format_unrecognized(""));
        // A non-empty, unrecognized shape trips the canary.
        assert!(is_agy_log_format_unrecognized(
            "[NEW FORMAT] upstream reshaped this line; no known signature"
        ));
        // Every known signature keeps classification on the recognized path.
        assert!(!is_agy_log_format_unrecognized(
            "RESOURCE_EXHAUSTED: quota exhausted"
        ));
        assert!(!is_agy_log_format_unrecognized(
            "not logged into Antigravity"
        ));
        assert!(!is_agy_log_format_unrecognized("Resets in 15m30s"));
    }

    #[test]
    fn agy_log_path_table_resolves_by_version() {
        // Unknown version falls back to every known layout's candidate.
        let cands = agy_log_candidates(None);
        assert!(cands.contains(&"cli.log"));
        assert!(cands.contains(&"log"));

        // A pre-1.0.0 version predates the rotated `log/` layout.
        let cands_old = agy_log_candidates(Some("0.9.0"));
        assert!(cands_old.contains(&"cli.log"));
        assert!(!cands_old.contains(&"log"));

        // A current version includes both layouts.
        let cands_new = agy_log_candidates(Some("1.0.16"));
        assert!(cands_new.contains(&"cli.log"));
        assert!(cands_new.contains(&"log"));

        // Candidates must be tried NEWEST-first: a version satisfying
        // multiple thresholds must not resolve the oldest matching layout
        // ahead of a newer one that's also a valid match.
        let cli_log_pos = cands_new.iter().position(|c| *c == "cli.log").unwrap();
        let log_dir_pos = cands_new.iter().position(|c| *c == "log").unwrap();
        assert!(
            log_dir_pos < cli_log_pos,
            "newer `log/` layout must be tried before the older flat `cli.log`, got {cands_new:?}"
        );
    }

    #[test]
    fn agy_cli_log_path_prefers_rotated_log_over_stale_flat_file() {
        // A stale `cli.log` left over from a pre-upgrade install, coexisting
        // with a freshly-populated `log/` directory the upgraded CLI is
        // actually writing to -- the resolved path must be the one inside
        // `log/`, not the dead flat file.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(".gemini/antigravity-cli");
        fs::create_dir_all(root.join("log")).unwrap();
        fs::write(root.join("cli.log"), "stale pre-upgrade content").unwrap();
        fs::write(root.join("log/cli-1.log"), "current rotated content").unwrap();

        let envs = vec![("HOME".to_string(), tmp.path().to_str().unwrap().to_string())];
        let resolved = agy_cli_log_path(&envs, Path::new("agy"), Some("1.0.16")).unwrap();
        assert_eq!(resolved, root.join("log/cli-1.log"));
    }

    #[test]
    fn run_agy_captures_cli_version_in_result() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 1.0.16'; else echo 'stdout-marker-agy'; fi\n",
        );
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();
        assert_eq!(result.agy_version.as_deref(), Some("1.0.16"));
    }

    #[test]
    fn run_agy_empty_output_with_unrecognized_log_emits_drift_note() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();

        // Fake AGY that reports a future version via `--version`, and on
        // `--print` exits 0 with no stdout but appends an *unrecognized* line
        // to its cli.log -- exactly the silent upstream log-format change
        // TICKET-242 defends against.
        let home = f._tmp.path().join("home");
        let cli_log = home.join(".gemini/antigravity-cli/cli.log");
        fs::create_dir_all(cli_log.parent().unwrap()).unwrap();
        fs::write(&cli_log, "").unwrap();
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 2.0.0'; elif [ \"$1\" = \"--print\" ]; then printf '[NEW FORMAT] upstream reshaped this line; no known signature\\n' >> '{}'; exit 0; else exit 1; fi\n",
            cli_log.display(),
        );
        make_fake_bin(&f.bin_dir, "agy", &body);

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), home.to_str().unwrap().to_string()),
        ];

        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, -1, "empty output must be a failure");
        assert_eq!(result.agy_version.as_deref(), Some("2.0.0"));

        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("agy_log_format_unrecognized"),
            "drift canary note must be present; log was:\n{log}"
        );
    }

    #[test]
    fn run_agy_empty_output_with_recognized_log_emits_no_drift_note() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();

        // Fake AGY that appends a *recognized* quota signature to its cli.log.
        let home = f._tmp.path().join("home");
        let cli_log = home.join(".gemini/antigravity-cli/cli.log");
        fs::create_dir_all(cli_log.parent().unwrap()).unwrap();
        fs::write(&cli_log, "").unwrap();
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 1.0.16'; elif [ \"$1\" = \"--print\" ]; then printf 'RESOURCE_EXHAUSTED: quota exhausted\\n' >> '{}'; exit 0; else exit 1; fi\n",
            cli_log.display(),
        );
        make_fake_bin(&f.bin_dir, "agy", &body);

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), home.to_str().unwrap().to_string()),
        ];

        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            !log.contains("agy_log_format_unrecognized"),
            "recognized signature must not trip the drift canary; log was:\n{log}"
        );
        assert!(
            log.contains("AGY quota exhausted"),
            "the recognized quota signature is still classified normally; log was:\n{log}"
        );
    }
}
