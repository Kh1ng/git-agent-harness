//! Issue #584: detect already-satisfied work before publishing regressive
//! completion MRs.
//!
//! A backend may return a structured `already_satisfied` disposition: the
//! source issue's requirements are already met in the target branch, backed by
//! grounded file/test evidence, with *no* repository diff. GAH must treat this
//! as a distinct fact from `agent_no_progress` (the agent genuinely tried and
//! could not make progress), and must not force an agent to manufacture a
//! change just to close an already-completed task.
//!
//! Reconciliation is provider-neutral (GitHub and GitLab behave identically):
//! when the profile is a trusted autonomous provider issue, GAH may post the
//! grounded evidence and close the issue idempotently; otherwise it emits a
//! bounded operator handoff rather than a regressive MR.
//!
//! A test-only diff that removes or weakens existing coverage must never be
//! accepted as proof that an already-implemented production task is complete.
//! That is treated as a regressive completion and is blocked from autonomous
//! closure.

use crate::config::Profile;

/// Grounded evidence that an issue's requirements are already satisfied in the
/// target branch. Every entry must point at a concrete artifact so a reviewer
/// (human or automated gate) can verify the claim without trusting the agent's
/// prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlreadySatisfiedEvidence {
    /// Concrete repository paths (files or directories) that already satisfy
    /// the requirement. Must be non-empty for a valid disposition.
    pub grounded_files: Vec<String>,
    /// Concrete test names or test file paths that demonstrate the requirement
    /// holds. Provider-neutral: either GitHub or GitLab.
    pub grounded_tests: Vec<String>,
    /// The repository diff produced while confirming satisfaction. `None`
    /// (no diff) is the expected, healthy case: GAH did not need to change
    /// anything. A non-empty diff is allowed only when it does not reduce
    /// coverage or otherwise regress the production task.
    pub repository_diff: Option<String>,
}

impl AlreadySatisfiedEvidence {
    /// A disposition is only valid when it carries at least one grounded
    /// pointer (a file or a test) and reports no repository diff. An
    /// `already_satisfied` claim without evidence, or one that quietly changed
    /// the repo, is not acceptable.
    pub fn is_grounded(&self) -> bool {
        (!self.grounded_files.is_empty() || !self.grounded_tests.is_empty())
            && self.repository_diff.is_none()
    }
}

/// A summarised view of a repository diff, supplied by the caller that owns
/// real git inspection. Kept pure so the reconciliation policy is fully
/// unit-testable without spawning git.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffSummary {
    /// Every changed file path.
    pub changed_files: Vec<String>,
    /// Subset of `changed_files` that are test files (heuristic: under a
    /// `tests/`/`test/` directory or matching `*_test`, `*_tests`,
    /// `*.test.*`). Used to reject test-only coverage removals.
    pub test_files: Vec<String>,
    /// Number of inserted lines.
    pub insertions: u32,
    /// Number of deleted lines.
    pub deletions: u32,
    /// True when the diff removes existing test coverage or weakens an
    /// assertion (e.g. deletes a test file, drops an assertion, or converts a
    /// `must`/`should` expectation into a comment). Detection is conservative:
    /// it flags the removal/weakening of test coverage, never the addition of
    /// new coverage.
    pub removes_or_weakens_coverage: bool,
}

impl DiffSummary {
    /// A diff that touches only test files and removes or weakens coverage is a
    /// regressive completion: it cannot count as finishing an
    /// already-implemented production task.
    pub fn is_test_only_coverage_regression(&self) -> bool {
        if !self.removes_or_weakens_coverage {
            return false;
        }
        if self.changed_files.is_empty() {
            return false;
        }
        self.changed_files.iter().all(|f| is_test_path_public(f))
    }
}

pub fn is_test_path_public(path: &str) -> bool {
    let lower = path.to_lowercase();
    if lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.starts_with("test/")
        || lower.contains("__tests__/")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".test.js")
        || lower.ends_with(".test.rs")
    {
        return true;
    }
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    file.ends_with("_test.rs")
        || file.ends_with("_tests.rs")
        || file.starts_with("test_")
        || file.ends_with("_test.py")
        || file == "tests.rs"
        || file == "tests.py"
}

/// The classified outcome of inspecting a backend's completion disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// The backend established, with grounded evidence and no repository diff,
    /// that the work is already satisfied. Distinct from `AgentNoProgress`.
    AlreadySatisfied(AlreadySatisfiedEvidence),
    /// The agent genuinely attempted the work but made no progress and produced
    /// no grounded satisfaction evidence. GAH must not force a manufactured
    /// change.
    AgentNoProgress,
    /// The backend produced a diff that only removes or weakens test coverage
    /// (a test-only regression). This is never accepted as the completion of
    /// an already-implemented production task.
    RegressiveCompletion(DiffSummary),
}

/// Classify a backend completion attempt.
///
/// * If `backend_summary` carries grounded satisfaction evidence and the diff
///   is empty/absent, this is `AlreadySatisfied`.
/// * If the diff is a test-only coverage regression, this is
///   `RegressiveCompletion` regardless of any claimed evidence — such a diff
///   must not be accepted as completing an already-implemented task.
/// * Otherwise, with no grounded satisfaction evidence and no acceptable diff,
///   this is `AgentNoProgress`.
pub fn classify_backend_disposition(backend_summary: &str, diff: &DiffSummary) -> Disposition {
    if diff.is_test_only_coverage_regression() {
        return Disposition::RegressiveCompletion(diff.clone());
    }

    let evidence = extract_evidence(backend_summary);
    if evidence.is_grounded() {
        return Disposition::AlreadySatisfied(evidence);
    }

    Disposition::AgentNoProgress
}

/// Extract grounded satisfaction evidence from a backend's structured summary.
///
/// The summary is expected to contain fenced evidence lines of the form
/// `file:<path>` and `test:<name>` (the same grounded-evidence grammar the
/// review gate already requires). A `diff:` line records a repository diff; its
/// presence marks the disposition as having changed the repo.
fn extract_evidence(summary: &str) -> AlreadySatisfiedEvidence {
    let mut grounded_files = Vec::new();
    let mut grounded_tests = Vec::new();
    let mut repository_diff = None;
    for line in summary.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("file:") {
            let path = rest.trim();
            if !path.is_empty() {
                grounded_files.push(path.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("test:") {
            let name = rest.trim();
            if !name.is_empty() {
                grounded_tests.push(name.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("diff:") {
            let body = rest.trim();
            if !body.is_empty() {
                repository_diff = Some(body.to_string());
            }
        }
    }
    AlreadySatisfiedEvidence {
        grounded_files,
        grounded_tests,
        repository_diff,
    }
}

/// The reconciliation outcome for an already-satisfied disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationDecision {
    /// The profile is a trusted autonomous provider issue: GAH may post the
    /// grounded evidence and close the source issue. `idempotent` is always
    /// true — re-running reconciliation must not produce a duplicate or error.
    PostEvidenceAndClose { idempotent: bool },
    /// GAH must not autonomously close the issue. Emit a bounded operator
    /// handoff describing exactly what evidence exists and why closure was
    /// withheld.
    BoundedOperatorHandoff { reason: String },
}

/// Whether `profile` represents a trusted autonomous provider issue that GAH
/// may reconcile autonomously. Provider-neutral: GitHub and GitLab are treated
/// identically; the operator grants closure authority via
/// `allow_source_issue_closure`. Any other provider, or a profile without
/// closure authorization, requires an operator handoff.
pub fn is_trusted_autonomous_provider(profile: &Profile) -> bool {
    matches!(profile.provider.as_str(), "github" | "gitlab")
        && profile.publishing.allow_source_issue_closure
}

/// Decide how to reconcile an already-satisfied disposition for `profile`.
///
/// For a trusted autonomous provider issue the decision posts the evidence and
/// closes the issue idempotently. Otherwise GAH emits a bounded operator
/// handoff rather than a regressive completion MR.
pub fn reconcile_already_satisfied(
    profile: &Profile,
    evidence: &AlreadySatisfiedEvidence,
) -> ReconciliationDecision {
    if !evidence.is_grounded() {
        return ReconciliationDecision::BoundedOperatorHandoff {
            reason: "already_satisfied disposition lacked grounded file/test evidence; refusing autonomous closure"
                .into(),
        };
    }
    if is_trusted_autonomous_provider(profile) {
        return ReconciliationDecision::PostEvidenceAndClose { idempotent: true };
    }
    ReconciliationDecision::BoundedOperatorHandoff {
        reason: format!(
            "profile '{}' (provider '{}') is not a trusted autonomous provider issue; operator must confirm closure",
            profile.display_name, profile.provider
        ),
    }
}

/// Issue #584: build a `DiffSummary` from a worktree for the reconciliation
/// check. Pure except for the (cheap, local) git inspection performed by
/// `worktree`. A test-only coverage regression is flagged conservatively: when
/// every changed file is a test file and the diff deletes lines. The caller
/// still owns the precise coverage-weakening signal.
pub(crate) fn build_diff_summary(wt: &std::path::Path, target_branch: &str) -> DiffSummary {
    let changed_files = crate::worktree::changed_files(wt, target_branch).unwrap_or_default();
    let test_files = changed_files
        .iter()
        .filter(|f| is_test_path_public(f))
        .cloned()
        .collect::<Vec<_>>();
    let stats =
        crate::worktree::diff_stats(wt, target_branch).unwrap_or(crate::worktree::DiffStats {
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        });
    let removes_or_weakens_coverage =
        !changed_files.is_empty() && test_files.len() == changed_files.len() && stats.deletions > 0;
    DiffSummary {
        changed_files,
        test_files,
        insertions: stats.insertions,
        deletions: stats.deletions,
        removes_or_weakens_coverage,
    }
}

/// Issue #584: emit a bounded, idempotent handoff for an already-satisfied
/// disposition that a trusted autonomous provider issue may close directly.
pub(crate) fn emit_already_satisfied_handoff(
    profile: &Profile,
    ledger: &crate::ledger::LedgerEntry,
    branch: &str,
    evidence: &AlreadySatisfiedEvidence,
) {
    println!("=== GAH already-satisfied reconciliation (idempotent close) ===");
    println!("profile: {}", profile.display_name);
    println!("branch: {}", branch);
    for file in &evidence.grounded_files {
        println!("grounded_file: {}", file);
    }
    for test in &evidence.grounded_tests {
        println!("grounded_test: {}", test);
    }
    println!(
        "validation_status: {}",
        ledger.validation_result.as_deref().unwrap_or("unknown")
    );
    println!("=== end GAH already-satisfied reconciliation ===");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::test_util::profile;

    fn github_profile() -> Profile {
        let mut p = profile(std::path::Path::new("/tmp/repo"));
        p.provider = "github".into();
        p
    }

    fn gitlab_profile() -> Profile {
        let mut p = profile(std::path::Path::new("/tmp/repo"));
        p.provider = "gitlab".into();
        p
    }

    #[test]
    fn failure_class_already_satisfied_serializes() {
        assert_eq!(
            crate::ledger::FailureClass::AlreadySatisfied.as_str(),
            "already_satisfied"
        );
    }

    #[test]
    fn already_satisfied_requires_grounded_evidence_and_no_diff() {
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(evidence.is_grounded());

        let empty = AlreadySatisfiedEvidence {
            grounded_files: vec![],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(!empty.is_grounded());

        let with_diff = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: Some("+ foo".into()),
        };
        assert!(!with_diff.is_grounded());
    }

    #[test]
    fn classify_prefers_already_satisfied_over_no_progress() {
        let summary = "Work is done.\nfile:src/foo.rs\ntest:tests::foo_works";
        let diff = DiffSummary::default();
        let d = classify_backend_disposition(summary, &diff);
        assert_eq!(
            d,
            Disposition::AlreadySatisfied(AlreadySatisfiedEvidence {
                grounded_files: vec!["src/foo.rs".into()],
                grounded_tests: vec!["tests::foo_works".into()],
                repository_diff: None,
            })
        );
    }

    #[test]
    fn classify_returns_agent_no_progress_without_evidence() {
        let summary = "I could not find a way to make progress.";
        let diff = DiffSummary::default();
        assert_eq!(
            classify_backend_disposition(summary, &diff),
            Disposition::AgentNoProgress
        );
    }

    #[test]
    fn classify_distinguishes_already_satisfied_from_no_progress() {
        let with_evidence = "Done.\nfile:src/foo.rs";
        let without = "Done.";
        assert!(matches!(
            classify_backend_disposition(with_evidence, &DiffSummary::default()),
            Disposition::AlreadySatisfied(_)
        ));
        assert_eq!(
            classify_backend_disposition(without, &DiffSummary::default()),
            Disposition::AgentNoProgress
        );
    }

    #[test]
    fn classify_rejects_test_only_coverage_regression() {
        let summary = "Removed dead tests.\nfile:tests/legacy.rs";
        let diff = DiffSummary {
            changed_files: vec!["tests/legacy.rs".into()],
            test_files: vec!["tests/legacy.rs".into()],
            insertions: 0,
            deletions: 40,
            removes_or_weakens_coverage: true,
        };
        assert_eq!(
            classify_backend_disposition(summary, &diff),
            Disposition::RegressiveCompletion(diff.clone())
        );
    }

    #[test]
    fn test_only_regression_is_rejected_even_with_evidence_claim() {
        let summary = "Already satisfied.\nfile:src/foo.rs";
        let diff = DiffSummary {
            changed_files: vec!["tests/legacy.rs".into()],
            test_files: vec!["tests/legacy.rs".into()],
            insertions: 0,
            deletions: 12,
            removes_or_weakens_coverage: true,
        };
        assert!(matches!(
            classify_backend_disposition(summary, &diff),
            Disposition::RegressiveCompletion(_)
        ));
    }

    #[test]
    fn trusted_github_profile_posts_and_closes_idempotently() {
        let mut p = github_profile();
        p.publishing.allow_source_issue_closure = true;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(is_trusted_autonomous_provider(&p));
        assert_eq!(
            reconcile_already_satisfied(&p, &evidence),
            ReconciliationDecision::PostEvidenceAndClose { idempotent: true }
        );
    }

    #[test]
    fn trusted_gitlab_profile_posts_and_closes_idempotently() {
        let mut p = gitlab_profile();
        p.publishing.allow_source_issue_closure = true;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec![],
            grounded_tests: vec!["tests::foo_works".into()],
            repository_diff: None,
        };
        assert!(is_trusted_autonomous_provider(&p));
        assert_eq!(
            reconcile_already_satisfied(&p, &evidence),
            ReconciliationDecision::PostEvidenceAndClose { idempotent: true }
        );
    }

    #[test]
    fn provider_without_closure_grant_emits_bounded_handoff() {
        let mut p = github_profile();
        p.publishing.allow_source_issue_closure = false;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(!is_trusted_autonomous_provider(&p));
        match reconcile_already_satisfied(&p, &evidence) {
            ReconciliationDecision::BoundedOperatorHandoff { reason } => {
                assert!(reason.contains("github"));
            }
            other => panic!("expected bounded handoff, got {other:?}"),
        }
    }

    #[test]
    fn non_github_gitlab_provider_requires_operator_handoff() {
        let mut p = github_profile();
        p.provider = "bitbucket".into();
        p.publishing.allow_source_issue_closure = true;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(!is_trusted_autonomous_provider(&p));
        assert!(matches!(
            reconcile_already_satisfied(&p, &evidence),
            ReconciliationDecision::BoundedOperatorHandoff { .. }
        ));
    }

    #[test]
    fn ungrounded_evidence_is_rejected_even_for_trusted_provider() {
        let mut p = github_profile();
        p.publishing.allow_source_issue_closure = true;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec![],
            grounded_tests: vec![],
            repository_diff: None,
        };
        assert!(matches!(
            reconcile_already_satisfied(&p, &evidence),
            ReconciliationDecision::BoundedOperatorHandoff { .. }
        ));
    }

    #[test]
    fn diff_present_blocks_autonomous_closure() {
        let mut p = github_profile();
        p.publishing.allow_source_issue_closure = true;
        let evidence = AlreadySatisfiedEvidence {
            grounded_files: vec!["src/foo.rs".into()],
            grounded_tests: vec![],
            repository_diff: Some("+ impl".into()),
        };
        assert!(matches!(
            reconcile_already_satisfied(&p, &evidence),
            ReconciliationDecision::BoundedOperatorHandoff { .. }
        ));
    }
}
