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
}
