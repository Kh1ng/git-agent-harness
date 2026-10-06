use super::*;

/// A fake backend CLI whose login check prints `status` and exits `code`.
#[cfg(unix)]
fn identity_with_login_check(
    dir: &std::path::Path,
    status: &str,
    code: i32,
) -> crate::execution_identity::ExecutionIdentity {
    use std::os::unix::fs::PermissionsExt;
    let executable = dir.join("codex");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh
echo '{status}'
exit {code}
"
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        Some("gpt-5"),
        None::<String>,
    );
    identity.set_executable(Some(executable));
    identity
}

/// Codex JSON output of a job that read a file about login failures.
#[cfg(unix)]
const WORK_OUTPUT_QUOTING_A_LOGIN_FAILURE: &str = r#"{"type":"item.completed","item":{"type":"command_execution","command":"cat docs/auth.md","aggregated_output":"setup reported: not logged in"}}"#;

#[cfg(unix)]
#[test]
fn login_failure_text_in_work_output_does_not_block_a_signed_in_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let identity = identity_with_login_check(tmp.path(), "Logged in using ChatGPT", 0);

    let parsed = mark_backend_unavailable_from_output_for_identity_at(
        &state,
        &identity,
        WORK_OUTPUT_QUOTING_A_LOGIN_FAILURE,
        "/tmp/backend-output.log",
    )
    .unwrap();

    assert!(parsed.is_none());
    assert!(
        !state.exists(),
        "nothing may be recorded against the backend"
    );
}

#[cfg(unix)]
#[test]
fn login_failure_still_blocks_when_the_login_check_agrees() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let identity = identity_with_login_check(tmp.path(), "Not logged in", 1);

    let parsed = mark_backend_unavailable_from_output_for_identity_at(
        &state,
        &identity,
        WORK_OUTPUT_QUOTING_A_LOGIN_FAILURE,
        "/tmp/backend-output.log",
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        parsed.kind,
        crate::quota_parser::FailureKind::AuthenticationError
    );
    assert!(fs::read_to_string(&state)
        .unwrap()
        .contains("authentication_error"));
}
