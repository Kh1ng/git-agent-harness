//! Phase 1 of the backend abstraction unification (issue #832): a real
//! trait for the dispatch path's one-shot spawn-and-collect shape, so
//! `dispatch/attempts.rs` stops hand-maintaining a `match` over six free
//! functions in `runner::backends::*`.
//!
//! Migrated one backend per PR, lowest risk first. Codex is first. Each
//! backend's existing free function (`run_<name>_with_executable`) stays as
//! the actual implementation and the tested regression net (golden-argv
//! unit tests in `backends/*.rs`) -- the trait impl is a thin adapter over
//! it, not a rewrite. `RunContext` is a superset bag of every backend's
//! parameters (`model` for codex/claude/vibe/opencode, `llm` for
//! agy/openhands, `print_timeout_seconds` for agy only); each impl reads
//! only the fields its underlying free function needs.
//!
//! Not covered here: the review path (`runner/review.rs`/
//! `runner/review_usage.rs`), which has a genuinely different call shape
//! (specialized idle-watch supervisor, usage-capture snapshot) -- see
//! issue #833.

use std::path::Path;

use anyhow::Result;

use crate::backend_kind::BackendKind;
use crate::runner::{LlmConfig, RunResult};

pub struct RunContext<'a> {
    pub executable: &'a Path,
    pub worktree: &'a Path,
    pub task: &'a str,
    pub session_dir: &'a Path,
    pub model: Option<&'a str>,
    pub llm: Option<&'a LlmConfig>,
    pub extra_args: &'a [String],
    pub env_vars: &'a [(String, String)],
    pub idle_timeout_seconds: u64,
    pub print_timeout_seconds: Option<u64>,
}

pub trait BackendRunner {
    fn kind(&self) -> BackendKind;
    fn run(&self, ctx: &RunContext) -> Result<RunResult>;
}

pub struct CodexRunner;

impl BackendRunner for CodexRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        crate::runner::run_codex_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            ctx.model,
            ctx.extra_args,
            ctx.env_vars,
            ctx.idle_timeout_seconds,
        )
    }
}

pub struct ClaudeRunner;

impl BackendRunner for ClaudeRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Claude
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        crate::runner::run_claude_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            ctx.model,
            ctx.extra_args,
            ctx.env_vars,
            ctx.idle_timeout_seconds,
        )
    }
}

pub struct VibeRunner;

impl BackendRunner for VibeRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Vibe
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        crate::runner::run_vibe_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            ctx.model,
            ctx.extra_args,
            ctx.env_vars,
            ctx.idle_timeout_seconds,
        )
    }
}

pub struct OpencodeRunner;

impl BackendRunner for OpencodeRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Opencode
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        crate::runner::run_opencode_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            ctx.model,
            ctx.extra_args,
            ctx.env_vars,
            ctx.idle_timeout_seconds,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    #[test]
    fn codex_runner_reports_codex_kind() {
        assert_eq!(CodexRunner.kind(), BackendKind::Codex);
    }

    #[test]
    fn codex_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec!["--trace".to_string()];

        let ctx = RunContext {
            executable: Path::new("codex"),
            worktree: &f.worktree,
            task: "the codex task",
            session_dir: &f.session_dir,
            model: Some("gpt-5.4"),
            llm: None,
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = CodexRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
        assert!(argv.contains(&"the codex task".to_string()));
        assert!(argv.contains(&"--trace".to_string()));
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
    }

    #[test]
    fn claude_runner_reports_claude_kind() {
        assert_eq!(ClaudeRunner.kind(), BackendKind::Claude);
    }

    #[test]
    fn claude_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec!["--add-dir".to_string(), "/tmp/extra".to_string()];

        let ctx = RunContext {
            executable: Path::new("claude"),
            worktree: &f.worktree,
            task: "the claude task",
            session_dir: &f.session_dir,
            model: Some("sonnet-5"),
            llm: None,
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = ClaudeRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the claude task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"sonnet-5".to_string()));
        assert!(argv.contains(&"--add-dir".to_string()));
        assert!(argv.contains(&"/tmp/extra".to_string()));
    }

    #[test]
    fn vibe_runner_reports_vibe_kind() {
        assert_eq!(VibeRunner.kind(), BackendKind::Vibe);
    }

    #[test]
    fn vibe_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec!["--custom-flag".to_string()];

        let ctx = RunContext {
            executable: Path::new("vibe"),
            worktree: &f.worktree,
            task: "the vibe task",
            session_dir: &f.session_dir,
            model: Some("devstral-small"),
            llm: None,
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = VibeRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the vibe task".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        assert!(argv.contains(&"--custom-flag".to_string()));
        let env = recorded_env(&f.record_dir);
        assert!(env.contains("VIBE_ACTIVE_MODEL=devstral-small"));
    }

    #[test]
    fn opencode_runner_reports_opencode_kind() {
        assert_eq!(OpencodeRunner.kind(), BackendKind::Opencode);
    }

    #[test]
    fn opencode_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec!["--custom-flag".to_string()];

        let ctx = RunContext {
            executable: Path::new("opencode"),
            worktree: &f.worktree,
            task: "the opencode task",
            session_dir: &f.session_dir,
            model: Some("glm-4.6"),
            llm: None,
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = OpencodeRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"the opencode task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"glm-4.6".to_string()));
        assert!(argv.contains(&"--custom-flag".to_string()));
    }
}
