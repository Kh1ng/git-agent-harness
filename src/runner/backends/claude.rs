use anyhow::Result;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::claude_monitor::find_claude_transcript;
use crate::runner::backends::write_refusal;
use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::filtered_backend_args;
use crate::runner::RunResult;

const DEFAULT_PERMISSION_MODE: &str = "acceptEdits";
const DEFAULT_ALLOWED_TOOLS: &str = "Edit,Write,MultiEdit,NotebookEdit,Bash";

/// Run Claude CLI non-interactively via `claude -p`.
/// extra_args come from profile.claude_args (e.g. `--allowedTools Edit,Write,Bash`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    // Issue #153: pin a stable session id so we can locate the exact
    // transcript `.jsonl` Claude Code writes afterwards (the source of real
    // per-attempt token/cost usage, rather than scraping stdout).
    let session_id = uuid::Uuid::new_v4().to_string();
    // Find the HOME this invocation will run under (a per-attempt HOME is
    // injected via env_vars; fall back to the ambient HOME).
    let home = env_vars
        .iter()
        .find_map(|(k, v)| (k == "HOME").then_some(PathBuf::from(v)))
        .or_else(|| env::var("HOME").ok().map(PathBuf::from));

    let mut cmd = Command::new(executable);
    cmd.args([
        "-p",
        task,
        "--output-format",
        "text",
        "--verbose",
        "--session-id",
        &session_id,
    ])
    .current_dir(worktree);
    if let Some(model) = effective_model {
        cmd.args(["--model", model]);
    }
    // Issue #1367: an implementation run must be able to edit the worktree
    // and run commands. Profile claude_args that set either flag win.
    let filtered_extra = filtered_backend_args("claude", extra_args);
    let has_flag = |names: &[&str]| {
        filtered_extra.iter().any(|arg| {
            names
                .iter()
                .any(|name| arg == name || arg.starts_with(&format!("{name}=")))
        })
    };
    let profile_sets_permission_mode =
        has_flag(&["--permission-mode", "--dangerously-skip-permissions"]);
    let profile_sets_allowed_tools = has_flag(&["--allowedTools", "--allowed-tools"]);
    if !profile_sets_permission_mode {
        cmd.args(["--permission-mode", DEFAULT_PERMISSION_MODE]);
    }
    if !profile_sets_allowed_tools {
        cmd.args(["--allowedTools", DEFAULT_ALLOWED_TOOLS]);
    }
    cmd.args(filtered_extra);
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let worktree_before = write_refusal::worktree_state(worktree);
    let (exit_code, duration_secs, resources) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching claude; is it installed and on PATH?",
    )?;

    // Locate the transcript for the pinned session id so per-attempt usage
    // parsing can consume it.
    let transcript_path = home
        .as_ref()
        .and_then(|h| find_claude_transcript(h, worktree, &session_id))
        .map(|p| p.to_string_lossy().into_owned());
    let transcript_text = transcript_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok());
    let final_summary = transcript_text
        .as_deref()
        .and_then(output::extract_claude_transcript_summary);

    // Issue #1367: a run whose write tools were denied and that left the
    // worktree untouched cannot succeed on retry. Runs GAH killed (negative
    // exit) keep their own classification.
    let refused_tools = transcript_text
        .as_deref()
        .map(write_refusal::claude_refused_write_tools)
        .unwrap_or_default();
    let exit_code = if exit_code >= 0
        && !refused_tools.is_empty()
        && !write_refusal::worktree_changed_since(worktree_before.as_deref(), worktree)
    {
        let cause = if profile_sets_permission_mode || profile_sets_allowed_tools {
            "profile claude_args overrides GAH's default --permission-mode/--allowedTools; make it grant these tools or remove the override".to_string()
        } else {
            format!(
                "GAH's defaults (--permission-mode {DEFAULT_PERMISSION_MODE} --allowedTools {DEFAULT_ALLOWED_TOOLS}) were not enough; check the Claude settings permissions (deny rules, managed policy) or set profile claude_args"
            )
        };
        write_refusal::report(
            &log_path,
            exit_code,
            &format!(
                "claude denied {} and changed nothing. {cause}.",
                refused_tools.join(", ")
            ),
        )
    } else {
        exit_code
    };

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path,
        agy_version: None,
        resources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    // ── run_claude ───────────────────────────────────────────────────────

    #[test]
    fn run_claude_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "claude task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-claude"));
        assert!(log.contains("stderr-marker-claude"));
    }

    #[test]
    fn run_claude_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_claude_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            None,
            &["--allowedTools".to_string(), "Edit,Bash".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the claude task".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(argv.contains(&"Edit,Bash".to_string()));
    }

    #[test]
    fn run_claude_grants_write_permissions_by_default() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv
            .windows(2)
            .any(|args| args == ["--permission-mode", "acceptEdits"]));
        assert!(argv
            .windows(2)
            .any(|args| args == ["--allowedTools", "Edit,Write,MultiEdit,NotebookEdit,Bash"]));
    }

    #[test]
    fn run_claude_profile_permission_args_replace_the_defaults() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[
                "--permission-mode".to_string(),
                "plan".to_string(),
                "--allowed-tools=Read".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv
            .windows(2)
            .any(|args| args == ["--permission-mode", "plan"]));
        assert!(argv.contains(&"--allowed-tools=Read".to_string()));
        assert!(!argv.contains(&"acceptEdits".to_string()));
        assert!(!argv.contains(&"--allowedTools".to_string()));
    }

    /// A fake `claude` that prints refusal-sounding prose, optionally edits
    /// the worktree, and writes a transcript for the pinned session id whose
    /// single `tool` call ends in `tool_result`.
    fn make_transcript_bin(f: &Fixture, tool: &str, tool_result: &str, edits_worktree: bool) {
        let call = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": tool, "input": {}}
            ]}
        });
        let outcome = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "is_error": true, "content": tool_result}
            ]}
        });
        let edit = if edits_worktree {
            "echo changed > progress.txt\n"
        } else {
            ""
        };
        make_fake_bin(
            &f.bin_dir,
            "claude",
            &format!(
                "#!/bin/sh\nsid=''\nwhile [ $# -gt 0 ]; do [ \"$1\" = '--session-id' ] && sid=\"$2\"; shift; done\n\
                 mkdir -p \"$HOME/.claude/projects/p\"\n\
                 cat > \"$HOME/.claude/projects/p/$sid.jsonl\" <<'GAH_EOF'\n{call}\n{outcome}\nGAH_EOF\n\
                 {edit}echo 'The ticket says the runner denies Edit and requires approval.'\n"
            ),
        );
    }

    fn transcript_envs(f: &Fixture) -> Vec<(String, String)> {
        vec![
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    f.bin_dir.to_str().unwrap(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            ),
            (
                "HOME".to_string(),
                f.record_dir.to_str().unwrap().to_string(),
            ),
        ]
    }

    const EDIT_DENIED: &str =
        "Claude requested permissions to write to /repo/src/lib.rs, but you haven't granted it yet.";

    #[test]
    fn run_claude_denied_write_without_changes_fails_as_configuration_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_transcript_bin(&f, "Edit", EDIT_DENIED, false);

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &transcript_envs(&f),
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        let detail = write_refusal::refusal_detail(&log).expect("marker line");
        assert!(detail.contains("claude denied Edit"), "got: {detail}");
        assert!(detail.contains("GAH's defaults"), "got: {detail}");
        assert!(detail.contains("claude_args"), "got: {detail}");
    }

    #[test]
    fn run_claude_denied_write_names_the_profile_override() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_transcript_bin(&f, "Edit", EDIT_DENIED, false);

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &["--permission-mode".to_string(), "plan".to_string()],
            &transcript_envs(&f),
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        let detail = write_refusal::refusal_detail(&log).expect("marker line");
        assert!(
            detail.contains("profile claude_args overrides"),
            "got: {detail}"
        );
    }

    #[test]
    fn run_claude_denied_write_does_not_fail_a_run_that_changed_the_worktree() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_transcript_bin(&f, "Edit", EDIT_DENIED, true);

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &transcript_envs(&f),
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(write_refusal::refusal_detail(&log).is_none());
    }

    #[test]
    fn run_claude_refusal_wording_in_output_does_not_fail_a_successful_run() {
        // The log quotes "denies Edit" / "requires approval", and a tool
        // failed for an ordinary reason: neither is a permission denial.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_transcript_bin(&f, "Edit", "String to replace not found in file.", false);

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "Fix #1367: the runner denies Edit and requires approval",
            &f.session_dir,
            None,
            &[],
            &transcript_envs(&f),
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("denies Edit"));
        assert!(write_refusal::refusal_detail(&log).is_none());
    }

    #[test]
    fn run_claude_binds_the_effective_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_claude_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[
                "--allowedTools".to_string(),
                "Edit".to_string(),
                "--model".to_string(),
                "opus".to_string(),
                "--model=sonnet".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(!argv.contains(&"opus".to_string()));
        assert!(!argv.contains(&"sonnet".to_string()));
        assert!(!argv.contains(&"--model=sonnet".to_string()));
    }

    #[test]
    fn run_claude_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "claude-env-value".to_string()),
        ];

        run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=claude-env-value"));
    }

    #[test]
    fn run_claude_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("launching claude; is it installed"));
    }

    #[test]
    fn run_claude_kills_process_after_idle_timeout_with_no_new_output() {
        // claude used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\necho 'step1'\nsleep 10\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_with_executable(
            Path::new("claude"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            3,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 3s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }
}
