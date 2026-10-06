use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::runner::backends::write_refusal;
use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::{codex_model_args, filtered_codex_args};
use crate::runner::{RunResult, WriteIntent};

/// Run Codex non-interactively via `codex exec`.
/// extra_args come from profile.codex_args, but stale model flags are
/// stripped so the resolved route controls the launched model.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
    write_intent: WriteIntent,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    let mut cmd = Command::new(executable);
    // Issue #152: --json produces structured JSONL output for programmatic
    // usage extraction in parse_codex_exec_json (usage.rs).
    cmd.arg("exec").arg("--json").arg(task);

    // Issue #1367: an implementation run must be able to write the worktree
    // and the build cache. Profile codex_args that choose a sandbox win.
    // Read-only job kinds run in the operator's real checkout and keep the
    // CLI's own sandbox default.
    let implementation = write_intent == WriteIntent::Implementation;
    let filtered_extra = filtered_codex_args(extra_args);
    let profile_sets_sandbox = filtered_extra.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-s" | "--sandbox" | "--full-auto" | "--dangerously-bypass-approvals-and-sandbox"
        ) || arg.starts_with("--sandbox=")
    });
    if implementation && !profile_sets_sandbox {
        cmd.arg("--sandbox").arg("workspace-write");
    }
    // The build writes only its own target directory, so that is all the
    // sandbox is opened for: never an ancestor, whose reach would depend on
    // how deep an operator-supplied CARGO_TARGET_DIR happens to be.
    if let Some(target) = env_vars
        .iter()
        .find(|(k, _)| implementation && k == "CARGO_TARGET_DIR")
        .map(|(_, v)| Path::new(v))
        .filter(|target| target.is_absolute() && target.parent().is_some())
    {
        cmd.arg("--add-dir").arg(target);
    }

    cmd.args(filtered_extra)
        .args(codex_model_args(model))
        .args(crate::execution_identity::selected_codex_config_args(
            env_vars,
        ))
        .current_dir(worktree);
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let worktree_before = write_refusal::worktree_state(worktree);
    let (exit_code, duration_secs, resources) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching codex; is it installed and on PATH?",
    )?;

    let output_text = fs::read_to_string(&log_path).unwrap_or_default();
    let transcript_path =
        crate::runner::review_usage::find_codex_transcript(env_vars, &output_text)
            .map(|path| path.to_string_lossy().into_owned());

    // Issue #1367: a run whose writes the sandbox rejected and that left the
    // worktree untouched cannot succeed on retry. Runs GAH killed (negative
    // exit) keep their own classification.
    let exit_code = if implementation
        && exit_code >= 0
        && write_refusal::codex_refused_writes(&output_text)
        && !write_refusal::worktree_changed_since(worktree_before.as_deref(), worktree)
    {
        let cause = if profile_sets_sandbox {
            "profile codex_args overrides GAH's default --sandbox workspace-write; make it allow workspace writes or remove the override"
        } else {
            "GAH's default --sandbox workspace-write was not enough; check the Codex config (sandbox_mode, approval policy) or set profile codex_args"
        };
        write_refusal::report(
            &log_path,
            exit_code,
            &format!("codex could not write to the worktree and changed nothing. {cause}."),
        )
    } else {
        exit_code
    };
    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: output::extract_codex_jsonl_summary(&output_text),
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
    // ── run_codex ────────────────────────────────────────────────────────

    #[test]
    fn run_codex_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "codex task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-codex"));
        assert!(log.contains("stderr-marker-codex"));
    }

    #[test]
    fn run_codex_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 7);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn run_codex_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "the codex task",
            &f.session_dir,
            None,
            &["-c".to_string(), "model=gpt".to_string()],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
        assert!(argv.contains(&"the codex task".to_string()));
        assert!(argv.contains(&"-c".to_string()));
        assert!(argv.contains(&"model=gpt".to_string()));
    }

    #[test]
    fn run_codex_grants_build_cache_with_add_dir() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let cache_root = f.worktree.join("build-cache");
        let target = cache_root.join("attempt-1/target");
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            (
                "CARGO_TARGET_DIR".to_string(),
                target.to_string_lossy().into_owned(),
            ),
        ];

        run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv
            .windows(2)
            .any(|args| args == ["--sandbox", "workspace-write"]));
        // Only the target directory itself: the cache root holds other
        // dispatches' targets, and a shallow target's ancestors could be the
        // home directory or `/`.
        let added: Vec<_> = argv
            .windows(2)
            .filter(|args| args[0] == "--add-dir")
            .map(|args| args[1].clone())
            .collect();
        assert_eq!(added, vec![target.to_string_lossy().into_owned()]);
        assert!(!argv.iter().any(|arg| arg.contains("allow-write-dir")));
    }

    #[test]
    fn run_codex_never_opens_the_sandbox_above_the_cargo_target() {
        for target in ["/target", "/", "target", ""] {
            let _exec_guard = crate::test_support::ExecGuard::new();
            let f = fixture();
            make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
            let envs = vec![
                ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
                ("CARGO_TARGET_DIR".to_string(), target.to_string()),
            ];

            run_with_executable(
                Path::new("codex"),
                &f.worktree,
                "task",
                &f.session_dir,
                None,
                &[],
                &envs,
                300,
                WriteIntent::Implementation,
            )
            .unwrap();

            let argv = recorded_argv(&f.record_dir);
            let added: Vec<_> = argv
                .windows(2)
                .filter(|args| args[0] == "--add-dir")
                .map(|args| args[1].as_str())
                .collect();
            // An absolute target below the root is granted as itself;
            // anything else is not granted at all.
            let expected: &[&str] = if target == "/target" {
                &["/target"]
            } else {
                &[]
            };
            assert_eq!(added, expected, "CARGO_TARGET_DIR={target:?}");
        }
    }

    #[test]
    fn run_codex_profile_sandbox_choice_replaces_the_default() {
        for profile_args in [
            vec!["-s", "read-only"],
            vec!["--sandbox=danger-full-access"],
            vec!["--full-auto"],
            vec!["--dangerously-bypass-approvals-and-sandbox"],
        ] {
            let _exec_guard = crate::test_support::ExecGuard::new();
            let f = fixture();
            make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
            let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
            let extra: Vec<String> = profile_args.iter().map(|arg| arg.to_string()).collect();

            run_with_executable(
                Path::new("codex"),
                &f.worktree,
                "task",
                &f.session_dir,
                None,
                &extra,
                &envs,
                300,
                WriteIntent::Implementation,
            )
            .unwrap();

            let argv = recorded_argv(&f.record_dir);
            assert!(
                !argv.contains(&"workspace-write".to_string()),
                "{profile_args:?} got {argv:?}"
            );
            assert!(argv.contains(&profile_args[0].to_string()));
        }
    }

    #[test]
    fn run_codex_read_only_dispatch_keeps_the_cli_sandbox_default() {
        // research/audit/estimate/pm run in the operator's real checkout.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            (
                "CARGO_TARGET_DIR".to_string(),
                f.worktree
                    .join("build-cache/attempt-1/target")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ];

        run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &["-c".to_string(), "model=gpt".to_string()],
            &envs,
            300,
            WriteIntent::ReadOnly,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(!argv.contains(&"--sandbox".to_string()), "got {argv:?}");
        assert!(!argv.contains(&"workspace-write".to_string()));
        assert!(!argv.contains(&"--add-dir".to_string()));
        assert!(argv.contains(&"model=gpt".to_string()));
    }

    #[test]
    fn run_codex_read_only_dispatch_does_not_report_rejected_writes() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        let envs = make_event_bin(&f, &[PATCH_REFUSED], false);

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::ReadOnly,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(write_refusal::refusal_detail(&log).is_none());
    }

    /// What codex-cli 0.160.0 prints when its sandbox rejects a patch: one
    /// stderr log line and no structured event.
    const PATCH_REFUSED: &str = "2026-10-06T15:36:04.664446Z ERROR codex_core::tools::router: error=patch rejected: writing is blocked by read-only sandbox; rejected by user approval settings";

    /// A fake `codex` that prints `events` as its `--json` stream and
    /// optionally edits the worktree.
    fn make_event_bin(f: &Fixture, events: &[&str], edits_worktree: bool) -> Vec<(String, String)> {
        let edit = if edits_worktree {
            "echo changed > progress.txt\n"
        } else {
            ""
        };
        make_fake_bin(
            &f.bin_dir,
            "codex",
            &format!(
                "#!/bin/sh\n{edit}cat <<'GAH_EOF'\n{}\nGAH_EOF\n",
                events.join("\n")
            ),
        );
        vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )]
    }

    #[test]
    fn run_codex_rejected_writes_without_changes_fail_as_configuration_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        let envs = make_event_bin(&f, &[PATCH_REFUSED], false);

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        let detail = write_refusal::refusal_detail(&log).expect("marker line");
        assert!(detail.contains("GAH's default --sandbox"), "got: {detail}");
        assert!(detail.contains("codex_args"), "got: {detail}");
    }

    #[test]
    fn run_codex_rejected_writes_name_the_profile_override() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        let envs = make_event_bin(&f, &[PATCH_REFUSED], false);

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &["--sandbox".to_string(), "read-only".to_string()],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        let detail = write_refusal::refusal_detail(&log).expect("marker line");
        assert!(
            detail.contains("profile codex_args overrides"),
            "got: {detail}"
        );
    }

    #[test]
    fn run_codex_rejected_patch_does_not_fail_a_run_that_changed_the_worktree() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        let envs = make_event_bin(&f, &[PATCH_REFUSED], true);

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(write_refusal::refusal_detail(&log).is_none());
    }

    #[test]
    fn run_codex_reading_refusal_wording_does_not_fail_a_successful_run() {
        // Reading GAH's own source or the issue text puts the sandbox phrase
        // and the marker itself into command output.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        let read_source = format!(
            r#"{{"type":"item.completed","item":{{"id":"i1","type":"command_execution","command":"cat src/runner/backends/write_refusal.rs","aggregated_output":"writing is blocked by read-only sandbox {}x","exit_code":0,"status":"completed"}}}}"#,
            write_refusal::WRITE_REFUSED_MARKER
        );
        let envs = make_event_bin(&f, &[&read_source], false);

        let result = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("writing is blocked by read-only sandbox"));
        assert!(write_refusal::refusal_detail(&log).is_none());
    }

    #[test]
    fn run_codex_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "codex-env-value".to_string()),
        ];

        run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap_err();

        assert!(err.to_string().contains("launching codex; is it installed"));
    }

    #[test]
    fn run_codex_kills_process_after_idle_timeout_with_no_new_output() {
        // codex used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "codex",
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
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            3,
            WriteIntent::Implementation,
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

    #[test]
    fn run_codex_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_with_executable(
            Path::new("codex"),
            &f.worktree,
            "task",
            &f.session_dir,
            Some("gpt-5.4"),
            &[
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-m".to_string(),
                "legacy-mini".to_string(),
                "--model=older".to_string(),
                "--trace".to_string(),
            ],
            &envs,
            300,
            WriteIntent::Implementation,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(argv.contains(&"--trace".to_string()));
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(!argv.contains(&"legacy-mini".to_string()));
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"--model=older".to_string()));
    }
}
