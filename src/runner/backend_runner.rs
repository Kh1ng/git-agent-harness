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
use std::time::Duration;

use anyhow::{Context, Result};

use crate::backend_kind::BackendKind;
use crate::runner::{LlmConfig, RunResult};

/// What a backend instance reports having, from a bounded self-report query
/// (issue #966). `Unknown` covers "this backend can't self-report" and
/// "the query failed/timed out" alike -- both must stay distinct from
/// `Skills(vec![])`, which means the backend was actually asked and
/// confirmed it has zero skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedSkills {
    Unknown,
    Skills(Vec<String>),
}

/// Parameters for a skill-inventory self-report query. `timeout` bounds the
/// query so a hung backend can never block a dispatch decision (#966 AC6).
/// `cwd`/`env_vars` let the caller target one specific backend instance's
/// isolated state/profile (mirroring every other Hermes invocation, which
/// always runs with an explicit working directory and environment) instead
/// of querying whatever instance gah's ambient environment happens to
/// resolve to.
pub struct SkillObservationContext<'a> {
    pub executable: &'a Path,
    pub timeout: Duration,
    pub cwd: Option<&'a Path>,
    pub env_vars: &'a [(String, String)],
}

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

    /// Ask this backend instance what skills it currently has (#966).
    /// Defaults to `Unknown` -- a backend gains real self-report only by
    /// overriding this, so a backend nobody has wired up yet reports
    /// "unknown" rather than silently claiming an empty inventory.
    fn observe_skills(&self, ctx: &SkillObservationContext) -> ObservedSkills {
        let _ = ctx;
        ObservedSkills::Unknown
    }
}

/// The one uniform constructor every `BackendKind` goes through. Callers
/// that need per-instance behavior (dispatch's `run`, or a skill-inventory
/// query) go through this instead of hand-writing their own match over
/// `BackendKind` (#966 AC1).
pub fn for_kind(kind: BackendKind) -> Box<dyn BackendRunner> {
    match kind {
        BackendKind::Claude => Box::new(ClaudeRunner),
        BackendKind::Codex => Box::new(CodexRunner),
        BackendKind::Opencode => Box::new(OpencodeRunner),
        BackendKind::Openhands => Box::new(OpenhandsRunner),
        BackendKind::Vibe => Box::new(VibeRunner),
        BackendKind::Agy => Box::new(AgyRunner),
        BackendKind::Hermes => Box::new(HermesRunner),
    }
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

pub struct HermesRunner;

impl BackendRunner for HermesRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Hermes
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        crate::runner::run_hermes_with_executable(
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

    fn observe_skills(&self, ctx: &SkillObservationContext) -> ObservedSkills {
        crate::runner::backends::hermes::observe_skills_with_executable(
            ctx.executable,
            ctx.cwd,
            ctx.env_vars,
            ctx.timeout,
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

pub struct OpenhandsRunner;

impl BackendRunner for OpenhandsRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Openhands
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        let llm = ctx.llm.context("openhands requires an LlmConfig")?;
        crate::runner::run_openhands_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            llm,
            ctx.extra_args,
            ctx.env_vars,
            ctx.idle_timeout_seconds,
        )
    }
}

pub struct AgyRunner;

impl BackendRunner for AgyRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Agy
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        let llm = ctx.llm.context("agy requires an LlmConfig")?;
        crate::runner::run_agy_with_executable(
            ctx.executable,
            ctx.worktree,
            ctx.task,
            ctx.session_dir,
            llm,
            ctx.env_vars,
            ctx.print_timeout_seconds,
            ctx.idle_timeout_seconds,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    // ── #966: uniform skill-inventory query ─────────────────────────────

    #[test]
    fn for_kind_covers_every_backend_kind_with_a_matching_runner() {
        // The call site (`for_kind`) branches once, centrally -- no caller
        // needs its own per-backend match to get a `BackendRunner` (#966
        // AC1). Prove every kind is wired, not just some.
        for kind in BackendKind::all() {
            assert_eq!(for_kind(kind).kind(), kind);
        }
    }

    #[test]
    fn default_observe_skills_is_unknown_never_an_empty_list() {
        let ctx = SkillObservationContext {
            executable: Path::new("does-not-matter"),
            timeout: Duration::from_secs(1),
            cwd: None,
            env_vars: &[],
        };
        // Every kind except Hermes uses the trait default today. A backend
        // nobody has wired self-report for must say "unknown", never claim
        // an empty inventory (#966 AC2).
        for kind in BackendKind::all() {
            if kind == BackendKind::Hermes {
                continue;
            }
            assert_eq!(
                for_kind(kind).observe_skills(&ctx),
                ObservedSkills::Unknown,
                "{kind:?} should default to Unknown"
            );
        }
    }

    #[test]
    fn hermes_runner_overrides_the_default_observe_skills() {
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "hermes",
            "#!/bin/sh\necho '{\"skills\": [\"review\"]}'\n",
        );
        let ctx = SkillObservationContext {
            executable: &f.bin_dir.join("hermes"),
            timeout: Duration::from_secs(5),
            cwd: None,
            env_vars: &[],
        };

        let observed = HermesRunner.observe_skills(&ctx);

        assert_eq!(observed, ObservedSkills::Skills(vec!["review".to_string()]));
    }

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
    fn hermes_runner_reports_hermes_kind() {
        assert_eq!(HermesRunner.kind(), BackendKind::Hermes);
    }

    #[test]
    fn hermes_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec![
            "--custom-flag".to_string(),
            "--skills".to_string(),
            "review".to_string(),
        ];

        let ctx = RunContext {
            executable: Path::new("hermes"),
            worktree: &f.worktree,
            task: "the hermes task",
            session_dir: &f.session_dir,
            model: Some("nous-portal/deepseek/deepseek-v4-flash"),
            llm: None,
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = HermesRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-z");
        assert!(argv.contains(&"the hermes task".to_string()));
        assert!(argv.contains(&"--yolo".to_string()));
        assert!(argv.contains(&"--accept-hooks".to_string()));
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"nous-portal/deepseek/deepseek-v4-flash".to_string()));
        assert!(argv.contains(&"--custom-flag".to_string()));
        assert!(argv.windows(2).any(|args| args == ["--skills", "review"]));
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

    #[test]
    fn openhands_runner_reports_openhands_kind() {
        assert_eq!(OpenhandsRunner.kind(), BackendKind::Openhands);
    }

    #[test]
    fn openhands_runner_requires_an_llm_config() {
        let f = fixture();
        let ctx = RunContext {
            executable: Path::new("openhands"),
            worktree: &f.worktree,
            task: "task",
            session_dir: &f.session_dir,
            model: None,
            llm: None,
            extra_args: &[],
            env_vars: &[],
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let err = OpenhandsRunner.run(&ctx).unwrap_err();

        assert!(err.to_string().contains("requires an LlmConfig"));
    }

    #[test]
    fn openhands_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let extra_args = vec!["--custom-flag".to_string()];
        let llm = LlmConfig {
            base_url: "https://llm.example".to_string(),
            api_key: "sk-test".to_string(),
            model: "claude-sonnet".to_string(),
        };

        let ctx = RunContext {
            executable: Path::new("openhands"),
            worktree: &f.worktree,
            task: "the openhands task",
            session_dir: &f.session_dir,
            model: None,
            llm: Some(&llm),
            extra_args: &extra_args,
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let result = OpenhandsRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--headless".to_string()));
        assert!(argv.contains(&"the openhands task".to_string()));
        assert!(argv.contains(&"--custom-flag".to_string()));
        let env = recorded_env(&f.record_dir);
        assert!(env.contains("LLM_MODEL=claude-sonnet"));
        assert!(env.contains("LLM_API_KEY=sk-test"));
    }

    #[test]
    fn agy_runner_reports_agy_kind() {
        assert_eq!(AgyRunner.kind(), BackendKind::Agy);
    }

    #[test]
    fn agy_runner_requires_an_llm_config() {
        let f = fixture();
        let ctx = RunContext {
            executable: Path::new("agy"),
            worktree: &f.worktree,
            task: "task",
            session_dir: &f.session_dir,
            model: None,
            llm: None,
            extra_args: &[],
            env_vars: &[],
            idle_timeout_seconds: 300,
            print_timeout_seconds: None,
        };

        let err = AgyRunner.run(&ctx).unwrap_err();

        assert!(err.to_string().contains("requires an LlmConfig"));
    }

    #[test]
    fn agy_runner_matches_the_free_function_it_wraps() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let llm = LlmConfig {
            base_url: "https://llm.example".to_string(),
            api_key: "sk-test".to_string(),
            model: "gemini-3-flash".to_string(),
        };

        let ctx = RunContext {
            executable: Path::new("agy"),
            worktree: &f.worktree,
            task: "the agy task",
            session_dir: &f.session_dir,
            model: None,
            llm: Some(&llm),
            extra_args: &[],
            env_vars: &envs,
            idle_timeout_seconds: 300,
            print_timeout_seconds: Some(900),
        };

        let result = AgyRunner.run(&ctx).unwrap();

        assert_eq!(result.exit_code, 0);
        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"the agy task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"gemini-3-flash".to_string()));
        assert!(argv.contains(&"--print-timeout".to_string()));
        assert!(argv.contains(&"900s".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
    }
}
