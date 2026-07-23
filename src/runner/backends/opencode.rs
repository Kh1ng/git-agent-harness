use anyhow::Result;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::process::{spawn_with_worktree_progress_watch, write_redacted_task};
use crate::runner::resolve::filtered_backend_args;
use crate::runner::{log_delta, RunResult};

/// Run OpenCode CLI non-interactively via `opencode run --model <model> --dir <path> --auto `<prompt>`.
/// Worker/fix backend and review backend support.
/// extra_args come from profile.opencode_args (e.g. `--format json`).
/// Unlike vibe, opencode DOES take --model, so we pass effective_model through.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_opencode(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_opencode_with_executable(
        Path::new("opencode"),
        worktree,
        task,
        session_dir,
        model,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

/// Issue #170: a live dispatch hung for 3+ hours with zero output and no
/// supervision at all -- opencode had no timeout of any kind (the previous
/// implementation used a plain blocking `cmd.status()`). Now uses the same
/// idle-detection approach as `run_agy_with_executable`: kill only when the
/// log has genuinely gone quiet for `idle_timeout_seconds`, never on a flat
/// wall-clock budget. OpenCode's own narration is not trusted as progress:
/// only a durable worktree change resets this backend's window.
#[allow(clippy::too_many_arguments)]
pub fn run_opencode_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;
    let started_at = std::time::SystemTime::now();
    // OpenCode emits provider-side failures (including the observed Hy3
    // rate-limit response) to this internal log, not reliably to stdout or
    // stderr. Snapshot its byte length before launch so only this attempt's
    // appended evidence can influence availability/routing.
    let opencode_log = opencode_log_path(env_vars);
    let opencode_log_pre_offset = opencode_log
        .as_ref()
        .and_then(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .unwrap_or(0);

    let mut cmd = Command::new(executable);
    // opencode run --model <model> --dir <path> --auto "<prompt>"
    cmd.arg("run").arg("--dir").arg(worktree).arg("--auto");

    // Add model if specified
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }

    // Add task as the last argument (prompt)
    cmd.arg(task);

    // Add extra args from profile
    cmd.args(filtered_backend_args("opencode", extra_args));

    cmd.current_dir(worktree);
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let (exit_code, duration_secs) = spawn_with_worktree_progress_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching opencode; is it installed and on PATH?",
    )?;
    let (transcript_path, final_summary) =
        snapshot_opencode_session(env_vars, worktree, started_at, session_dir)
            .map(|snapshot| (Some(snapshot.path), snapshot.final_summary))
            .unwrap_or((None, None));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: log_delta(&opencode_log, opencode_log_pre_offset),
        internal_log_path: opencode_log.map(|path| path.to_string_lossy().into_owned()),
        transcript_path,
        agy_version: None,
    })
}

/// Locate OpenCode's process-wide diagnostic log using the same data-home
/// resolution as its SQLite session store. The configured per-run HOME/XDG
/// environment wins over the parent process, which keeps isolated backend
/// instances from reading each other's diagnostics.
fn opencode_log_path(env_vars: &[(String, String)]) -> Option<PathBuf> {
    let value_for = |name: &str| {
        env_vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| env::var(name).ok())
    };
    value_for("XDG_DATA_HOME")
        .map(|path| PathBuf::from(path).join("opencode/log/opencode.log"))
        .or_else(|| {
            value_for("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/opencode/log/opencode.log"))
        })
}

/// Persist the exact OpenCode session created by this invocation as a small
/// JSON artifact. Querying by worktree and start time prevents concurrent
/// workers from attributing each other's SQLite rows.
pub(crate) struct OpenCodeSessionSnapshot {
    pub(crate) path: String,
    pub(crate) final_summary: Option<String>,
}

pub(crate) fn snapshot_opencode_session(
    env_vars: &[(String, String)],
    worktree: &Path,
    started_at: std::time::SystemTime,
    session_dir: &Path,
) -> Option<OpenCodeSessionSnapshot> {
    let value_for = |name: &str| {
        env_vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| env::var(name).ok())
    };
    let database = value_for("XDG_DATA_HOME")
        .map(|path| PathBuf::from(path).join("opencode/opencode.db"))
        .or_else(|| {
            value_for("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/opencode/opencode.db"))
        })?;
    let started_at_ms = started_at
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    // OpenCode 1.17+ reports the provider-computed session dollar cost. Keep
    // older databases readable by selecting NULL when that column is absent.
    let cost_expression = if connection
        .prepare("SELECT cost FROM session LIMIT 0")
        .is_ok()
    {
        "cost"
    } else {
        "NULL"
    };
    let query = format!(
        "SELECT id, model, tokens_input, tokens_output, tokens_reasoning, \
         tokens_cache_read, tokens_cache_write, {cost_expression}, time_updated \
         FROM session WHERE directory = ?1 AND time_created >= ?2 \
         ORDER BY time_updated DESC LIMIT 1"
    );
    let mut statement = connection.prepare(&query).ok()?;
    let snapshot = statement
        .query_row(
            rusqlite::params![worktree.to_string_lossy(), started_at_ms],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "model": serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(1)?)
                        .unwrap_or(serde_json::Value::Null),
                    "tokens_input": row.get::<_, i64>(2)? as u64,
                    "tokens_output": row.get::<_, i64>(3)? as u64,
                    "tokens_reasoning": row.get::<_, i64>(4)? as u64,
                    "tokens_cache_read": row.get::<_, i64>(5)? as u64,
                    "tokens_cache_write": row.get::<_, i64>(6)? as u64,
                    "actual_cost_usd": row.get::<_, Option<f64>>(7)?,
                    "time_updated": row.get::<_, i64>(8)?,
                }))
            },
        )
        .ok()?;
    let session_id = snapshot.get("id")?.as_str()?;
    let final_summary = connection
        .prepare(
            "SELECT message.data, part.data FROM message \
             JOIN part ON part.message_id = message.id \
             WHERE message.session_id = ?1 \
             ORDER BY message.time_created DESC, part.time_created DESC",
        )
        .ok()
        .and_then(|mut statement| {
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .ok()?;
            for row in rows.flatten() {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&row.0) else {
                    continue;
                };
                let Ok(part) = serde_json::from_str::<serde_json::Value>(&row.1) else {
                    continue;
                };
                if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                    && message.get("finish").and_then(serde_json::Value::as_str) == Some("stop")
                    && part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                {
                    if let Some(text) = part
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        return Some(text.to_string());
                    }
                }
            }
            None
        });
    let path = session_dir.join("opencode-session.json");
    fs::write(&path, serde_json::to_vec(&snapshot).ok()?).ok()?;
    Some(OpenCodeSessionSnapshot {
        path: path.to_string_lossy().into_owned(),
        final_summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;

    #[test]
    fn run_opencode_core_argv_includes_run_dir_auto_and_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            Some("provider/test-model"),
            &["--format".to_string(), "json".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&f.worktree.to_string_lossy().to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"provider/test-model".to_string()));
        assert!(argv.contains(&"--format".to_string()));
        assert!(argv.contains(&"json".to_string()));
        assert!(argv.contains(&"the opencode task".to_string()));
    }

    #[test]
    fn run_opencode_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            Some("provider/test-model"),
            &[
                "--model".to_string(),
                "stale-model".to_string(),
                "--model=another-stale".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"provider/test-model".to_string()));
        assert!(argv.contains(&"--format".to_string()));
        assert!(argv.contains(&"json".to_string()));
        assert!(!argv.contains(&"stale-model".to_string()));
        assert!(!argv.contains(&"another-stale".to_string()));
        assert!(!argv.contains(&"--model=another-stale".to_string()));
    }

    #[test]
    fn run_opencode_without_model_still_includes_run_dir_auto() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&f.worktree.to_string_lossy().to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"the opencode task".to_string()));
        // No --model flag should be present when model is None
        assert!(!argv.contains(&"--model".to_string()));
    }

    #[test]
    fn run_opencode_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            (
                "FROM_ENV_FILE".to_string(),
                "opencode-env-value".to_string(),
            ),
        ];

        run_opencode(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=opencode-env-value"));
    }

    #[test]
    fn snapshot_opencode_session_scopes_metadata_to_worktree_and_start_time() {
        let f = fixture();
        let data_dir = f._tmp.path().join(".local/share/opencode");
        fs::create_dir_all(&data_dir).unwrap();
        let database = data_dir.join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    directory TEXT NOT NULL,
                    model TEXT NOT NULL,
                    tokens_input INTEGER NOT NULL,
                    tokens_output INTEGER NOT NULL,
                    tokens_reasoning INTEGER NOT NULL,
                    tokens_cache_read INTEGER NOT NULL,
                    tokens_cache_write INTEGER NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );",
            )
            .unwrap();
        let started_at = std::time::SystemTime::now();
        let started_at_ms = started_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        connection
            .execute(
                "INSERT INTO session VALUES (?1, ?2, ?3, 775, 140, 20, 15360, 0, ?4, ?5)",
                rusqlite::params![
                    "session-current",
                    f.worktree.to_string_lossy(),
                    r#"{"id":"hy3-free","providerID":"opencode"}"#,
                    started_at_ms + 1,
                    started_at_ms + 2,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "message-final",
                    "session-current",
                    started_at_ms + 3,
                    r#"{"role":"assistant","finish":"stop"}"#,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    "part-final",
                    "message-final",
                    "session-current",
                    started_at_ms + 4,
                    r#"{"type":"text","text":"Implemented the scoped fix."}"#,
                ],
            )
            .unwrap();
        let envs = vec![("HOME".to_string(), f._tmp.path().display().to_string())];

        let snapshot = snapshot_opencode_session(&envs, &f.worktree, started_at, &f.session_dir)
            .expect("current worktree session should be captured");
        let usage = crate::usage::parse_opencode_session_metadata(
            &fs::read_to_string(snapshot.path).unwrap(),
        );
        assert_eq!(usage.actual_model.as_deref(), Some("hy3-free"));
        assert_eq!(usage.input_tokens, Some(775));
        assert_eq!(usage.total_tokens, Some(16295));
        assert_eq!(
            snapshot.final_summary.as_deref(),
            Some("Implemented the scoped fix.")
        );
    }

    #[test]
    fn run_opencode_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err =
            run_opencode(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching opencode; is it installed"));
    }

    #[test]
    fn run_opencode_kills_process_after_idle_timeout_with_no_new_output() {
        // Issue #170: a live opencode dispatch hung for 3+ hours with zero
        // output and no supervision at all -- opencode previously used a
        // plain blocking `cmd.status()`. This pins the fix: it must be
        // killed once output has genuinely stopped, not allowed to run
        // forever.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            1, // idle timeout: 1s of silence is stalled
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_opencode_kills_chatty_backend_without_worktree_progress() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\nwhile true; do echo 'tool chatter with no successful edit'; sleep 1; done\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            1,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("tool chatter with no successful edit"));
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_opencode_allows_real_worktree_progress_despite_chatty_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\necho 'starting edit'; sleep 1; printf 'first\\n' > progress.txt; echo 'editing'; sleep 1; printf 'second\\n' > progress.txt; echo 'done'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            3,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            fs::read_to_string(f.worktree.join("progress.txt")).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn run_opencode_captures_only_its_internal_log_delta() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let data_home = f._tmp.path().join("xdg-data");
        let internal_log = data_home.join("opencode/log/opencode.log");
        fs::create_dir_all(internal_log.parent().unwrap()).unwrap();
        fs::write(
            &internal_log,
            "old run: AI_APICallError: Rate limit exceeded. Please try again later.\n",
        )
        .unwrap();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\nprintf '%s\\n' 'timestamp=now level=ERROR message=\"AI_APICallError: Rate limit exceeded. Please try again later.\"' >> \"$XDG_DATA_HOME/opencode/log/opencode.log\"\nexit 0\n",
        );
        let envs = vec![
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    f.bin_dir.to_str().unwrap(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            ),
            ("XDG_DATA_HOME".to_string(), data_home.display().to_string()),
        ];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            Some("opencode/hy3-free"),
            &[],
            &envs,
            5,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.internal_log_path.as_deref(),
            Some(internal_log.to_str().unwrap())
        );
        let delta = result.internal_log_delta.as_deref().unwrap();
        assert!(delta.contains("Rate limit exceeded"));
        assert!(!delta.contains("old run"), "delta was {delta:?}");
    }
}
