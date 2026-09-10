//! Golden contract tests through the PRODUCTION dispatch path.
//!
//! These are the highest-value regression guards in the runner layer: each
//! test drives a recording fake binary through `for_kind(kind).run(&ctx)` —
//! the exact path dispatch uses — and pins the CLI invocation shape (argv),
//! stream handling, and result collection. Any change that drifts a
//! backend's argv, breaks its log/summary plumbing, or mis-threads a
//! RunContext field fails here before it can reach a real agent CLI.
//!
//! The per-backend test modules in `backends/*.rs` cover the same contracts
//! through the crate-level core functions plus deeper behaviors (stall
//! kills, usage extraction, transcript binding). Both layers matter: this
//! one pins the wiring, those pin the details.

use super::backend_runner::for_kind;
use super::{LlmConfig, RunContext};
use crate::backend_kind::BackendKind;
use crate::runner::backends::test_util::*;
use std::fs;
use std::path::Path;

struct CtxArgs<'a> {
    executable: &'a Path,
    worktree: &'a Path,
    task: &'a str,
    session_dir: &'a Path,
    model: Option<&'a str>,
    llm: Option<&'a LlmConfig>,
    extra_args: &'a [String],
    env_vars: &'a [(String, String)],
    idle_timeout_seconds: u64,
}

fn ctx(a: CtxArgs<'_>) -> RunContext<'_> {
    RunContext {
        executable: a.executable,
        worktree: a.worktree,
        task: a.task,
        session_dir: a.session_dir,
        model: a.model,
        llm: a.llm,
        extra_args: a.extra_args,
        env_vars: a.env_vars,
        idle_timeout_seconds: a.idle_timeout_seconds,
        print_timeout_seconds: None,
    }
}

/// Drive one backend turn through the production path and hand back the
/// recorded argv, the run result, and the captured log content. The log is
/// read inside this helper because the fixture's temp dir dies with it.
fn run_through_runner(
    kind: BackendKind,
    tool: &str,
    task: &str,
) -> (Vec<String>, super::RunResult, String) {
    let f = fixture();
    let _exec_guard = crate::test_support::ExecGuard::new();
    make_recording_bin(&f.bin_dir, tool, &f.record_dir, 0);
    let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
    let llm = test_llm();
    let uses_llm = matches!(kind, BackendKind::Agy | BackendKind::Openhands);
    let result = for_kind(kind)
        .run(&ctx(CtxArgs {
            executable: &f.bin_dir.join(tool),
            worktree: &f.worktree,
            task,
            session_dir: &f.session_dir,
            model: None,
            llm: uses_llm.then_some(&llm),
            extra_args: &[],
            env_vars: &envs,
            idle_timeout_seconds: 300,
        }))
        .expect("runner run must succeed");
    let log = fs::read_to_string(&result.log_path).unwrap_or_default();
    let argv = recorded_argv(&f.record_dir);
    (argv, result, log)
}

#[test]
fn contract_codex_runner_invokes_exec_json() {
    let (argv, result, log) = run_through_runner(BackendKind::Codex, "codex", "codex task");
    assert_eq!(result.exit_code, 0);
    assert_eq!(argv[0], "exec");
    assert_eq!(argv[1], "--json");
    assert!(argv.contains(&"codex task".to_string()));
    assert!(log.contains("stdout-marker-codex"));
}

#[test]
fn contract_claude_runner_invokes_print_mode() {
    let (argv, result, log) = run_through_runner(BackendKind::Claude, "claude", "claude task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"-p".to_string()));
    assert!(argv.contains(&"claude task".to_string()));
    assert!(log.contains("stdout-marker-claude"));
}

#[test]
fn contract_hermes_runner_invokes_task_yolo() {
    let (argv, result, log) = run_through_runner(BackendKind::Hermes, "hermes", "hermes task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"-z".to_string()));
    assert!(argv.contains(&"hermes task".to_string()));
    assert!(argv.contains(&"--yolo".to_string()));
    assert!(log.contains("stdout-marker-hermes"));
}

#[test]
fn contract_vibe_runner_invokes_trust_auto_approve() {
    let (argv, result, log) = run_through_runner(BackendKind::Vibe, "vibe", "vibe task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"-p".to_string()));
    assert!(argv.contains(&"vibe task".to_string()));
    assert!(argv.contains(&"--trust".to_string()));
    assert!(argv.contains(&"--auto-approve".to_string()));
    assert!(log.contains("stdout-marker-vibe"));
}

#[test]
fn contract_opencode_runner_selects_implementer_agent() {
    let (argv, result, log) =
        run_through_runner(BackendKind::Opencode, "opencode", "opencode task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"run".to_string()));
    assert!(argv.contains(&"--agent".to_string()));
    assert!(argv.contains(&"gah-implementer".to_string()));
    assert!(argv.contains(&"opencode task".to_string()));
    assert!(log.contains("stdout-marker-opencode"));
}

#[test]
fn contract_openhands_runner_invokes_headless() {
    let (argv, result, log) =
        run_through_runner(BackendKind::Openhands, "openhands", "openhands task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"--headless".to_string()));
    assert!(argv.contains(&"--json".to_string()));
    assert!(argv.contains(&"openhands task".to_string()));
    assert!(argv.contains(&"--exit-without-confirmation".to_string()));
    assert!(log.contains("stdout-marker-openhands"));
}

#[test]
fn contract_agy_runner_invokes_print_stream_json() {
    let (argv, result, log) = run_through_runner(BackendKind::Agy, "agy", "agy task");
    assert_eq!(result.exit_code, 0);
    assert!(argv.contains(&"--print".to_string()));
    assert!(argv.contains(&"--output-format".to_string()));
    assert!(argv.contains(&"stream-json".to_string()));
    assert!(argv.contains(&"agy task".to_string()));
    assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(log.contains("stdout-marker-agy"));
}
