use std::fs;

pub(crate) mod backends;
pub(crate) mod output;
pub(crate) mod process;
pub(crate) mod resolve;
pub(crate) mod review;
pub(crate) mod review_usage;

#[allow(unused_imports)]
pub(crate) use crate::runner::backends::agy::log_delta;
#[allow(unused_imports)]
pub use crate::runner::backends::agy::{run_agy, run_agy_with_executable};
#[allow(unused_imports)]
pub use crate::runner::backends::claude::{run_claude, run_claude_with_executable};
#[allow(unused_imports)]
pub use crate::runner::backends::codex::{run_codex, run_codex_with_executable};
#[allow(unused_imports)]
pub use crate::runner::backends::opencode::{run_opencode, run_opencode_with_executable};
#[allow(unused_imports)]
pub use crate::runner::backends::openhands::{list_oh_profiles, load_oh_profile, run_openhands};
#[allow(unused_imports)]
pub use crate::runner::backends::vibe::{run_vibe, run_vibe_with_executable};
#[allow(unused_imports)]
pub(crate) use crate::runner::process::{
    copy_stream_to_file, kill_process_group, prepare_process_group, spawn_with_idle_watch,
    spawn_with_worktree_progress_watch, write_redacted_task,
};
#[allow(unused_imports)]
pub use crate::runner::process::{install_shutdown_handler, shutdown_requested};
#[allow(unused_imports)]
pub use crate::runner::resolve::{
    backend_available, backend_available_for_profile, codex_model_args, extract_model_from_args,
    extract_model_from_backend_args, filtered_backend_args, filtered_codex_args,
    require_backend_executable, resolve_backend_executable, ExecutableResolution,
};
#[allow(unused_imports)]
pub use crate::runner::review::{run_review_backend, ReviewProcessOutcome, ReviewRunResult};

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
