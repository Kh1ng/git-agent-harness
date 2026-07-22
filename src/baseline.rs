//! TICKET-110: classify why baseline validation failed on the pristine
//! worktree, before any attempt spends tokens. Deterministic and pure — no
//! LLM call, no network call, so "do not let an LLM improvise baseline
//! ownership" (TICKET-111) holds by construction.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineDisposition {
    /// Baseline validation passed.
    Clean,
    /// Baseline fails, but the profile explicitly declared this failure as
    /// known/expected via `known_failure_markers` — never inferred.
    ExpectedRed,
    /// The validation command itself could not run (POSIX shell exit 127
    /// "command not found" or 126 "found but not executable") — this is a
    /// harness/environment-setup problem, not a code problem.
    HarnessError,
    /// The command ran but failed on a well-known dependency/connectivity
    /// signature (missing module, linker not found, connection refused).
    EnvironmentError,
    /// Validation failed and nothing above matched. The only safe default —
    /// never silently promoted to `ExpectedRed`.
    UnknownRed,
}

impl BaselineDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::ExpectedRed => "expected_red",
            Self::HarnessError => "harness_error",
            Self::EnvironmentError => "environment_error",
            Self::UnknownRed => "unknown_red",
        }
    }
}

/// Small, explicitly justified set of dependency/connectivity failure
/// signatures common across ecosystems (Python, Node, Rust, generic
/// networking). Each is a well-known, unambiguous string emitted by the
/// tool itself, not a guess — see the ticket's "no speculative patterns"
/// constraint. Kept short deliberately; expand only with a concrete example.
const ENVIRONMENT_ERROR_SIGNATURES: &[&str] = &[
    "ModuleNotFoundError",             // Python: import target not installed
    "No module named",                 // Python: alternate phrasing
    "Cannot find module",              // Node.js: require()/import target not installed
    "error: linking with",             // Rust: linker (cc/ld) missing or misconfigured
    "Connection refused",              // generic: dependent service (DB, API) not reachable
    "ECONNREFUSED",                    // Node.js: same, as an error code
    "No such file or directory: 'cc'", // Rust: no C toolchain installed
    "Text file busy",                  // ETXTBSY: concurrent exec of a binary mid-write (#568)
];

/// `exit_code` is the POSIX shell exit status of the *validation command
/// itself* (via `sh -c`), not something parsed from its stdout/stderr.
/// `known_failure_markers` are case-insensitive substrings the profile has
/// explicitly configured as expected-red signatures.
pub fn classify_baseline(
    text: &str,
    exit_code: Option<i32>,
    known_failure_markers: &[String],
) -> BaselineDisposition {
    if text.is_empty() && exit_code.is_none() {
        return BaselineDisposition::Clean;
    }

    if matches!(exit_code, Some(126) | Some(127)) {
        return BaselineDisposition::HarnessError;
    }

    if matches!(
        exit_code,
        Some(crate::validation_runner::VALIDATION_COMMAND_TIMEOUT_EXIT_CODE)
    ) {
        return BaselineDisposition::HarnessError;
    }

    for marker in known_failure_markers {
        if !marker.is_empty() && text.to_lowercase().contains(&marker.to_lowercase()) {
            return BaselineDisposition::ExpectedRed;
        }
    }

    for signature in ENVIRONMENT_ERROR_SIGNATURES {
        if text.contains(signature) {
            return BaselineDisposition::EnvironmentError;
        }
    }

    BaselineDisposition::UnknownRed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_failure_text_is_clean() {
        assert_eq!(classify_baseline("", None, &[]), BaselineDisposition::Clean);
    }

    #[test]
    fn exit_127_is_harness_error_regardless_of_text() {
        assert_eq!(
            classify_baseline("anything at all", Some(127), &[]),
            BaselineDisposition::HarnessError
        );
    }

    #[test]
    fn exit_126_is_harness_error() {
        assert_eq!(
            classify_baseline("Permission denied", Some(126), &[]),
            BaselineDisposition::HarnessError
        );
    }

    #[test]
    fn configured_marker_is_expected_red() {
        let markers = vec!["known flaky integration test".to_string()];
        assert_eq!(
            classify_baseline(
                "FAILED tests/test_x.py - known flaky integration test",
                Some(1),
                &markers,
            ),
            BaselineDisposition::ExpectedRed
        );
    }

    #[test]
    fn marker_match_is_case_insensitive() {
        let markers = vec!["KNOWN FLAKY".to_string()];
        assert_eq!(
            classify_baseline("known flaky test failure", Some(1), &markers),
            BaselineDisposition::ExpectedRed
        );
    }

    #[test]
    fn unconfigured_text_never_becomes_expected_red() {
        // No markers configured -> even a very "expected-sounding" failure
        // must not be auto-promoted to ExpectedRed.
        assert_eq!(
            classify_baseline("known flaky integration test", Some(1), &[]),
            BaselineDisposition::UnknownRed
        );
    }

    #[test]
    fn python_module_not_found_is_environment_error() {
        assert_eq!(
            classify_baseline(
                "ModuleNotFoundError: No module named 'requests'",
                Some(1),
                &[],
            ),
            BaselineDisposition::EnvironmentError
        );
    }

    #[test]
    fn node_cannot_find_module_is_environment_error() {
        assert_eq!(
            classify_baseline("Error: Cannot find module 'express'", Some(1), &[]),
            BaselineDisposition::EnvironmentError
        );
    }

    #[test]
    fn text_file_busy_is_environment_error() {
        assert_eq!(
            classify_baseline(
                "fatal: cannot exec '.git/hooks/fake-git': Text file busy",
                Some(1),
                &[],
            ),
            BaselineDisposition::EnvironmentError
        );
    }

    #[test]
    fn rust_linker_failure_is_environment_error() {
        assert_eq!(
            classify_baseline(
                "error: linking with `cc` failed: exit status: 1",
                Some(101),
                &[],
            ),
            BaselineDisposition::EnvironmentError
        );
    }

    #[test]
    fn connection_refused_is_environment_error() {
        assert_eq!(
            classify_baseline(
                "psycopg2.OperationalError: Connection refused",
                Some(1),
                &[]
            ),
            BaselineDisposition::EnvironmentError
        );
    }

    #[test]
    fn generic_assertion_failure_is_unknown_red() {
        assert_eq!(
            classify_baseline("AssertionError: expected 5 got 4", Some(1), &[],),
            BaselineDisposition::UnknownRed
        );
    }

    #[test]
    fn expected_red_marker_takes_precedence_over_environment_signature() {
        // If a profile explicitly marks a failure as expected, that wins
        // even if the text also happens to contain an environment-error
        // signature -- explicit configuration is authoritative.
        let markers = vec!["module x is expected to be missing in ci".to_string()];
        assert_eq!(
            classify_baseline(
                "ModuleNotFoundError: module x is expected to be missing in CI",
                Some(1),
                &markers,
            ),
            BaselineDisposition::ExpectedRed
        );
    }
}
