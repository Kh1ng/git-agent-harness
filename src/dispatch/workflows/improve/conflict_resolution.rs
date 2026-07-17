use super::super::super::attempts::{attempt_usage, classify_git_operation_result};
use super::super::super::prompts::indent_untrusted_text;
use super::super::super::text::utf8_safe_prefix;
use crate::ledger::{AttemptRecord, FailureClass, FailureStage, LedgerEntry};
use crate::routing::RouteDecision;
use crate::runner::RunResult;
use crate::usage_attribution::UsageAttribution;
use crate::worktree::{self, TargetRefreshOutcome};
use anyhow::Result;
use std::path::Path;

const MAX_PROMPT_CONFLICT_FILES: usize = 64;
const MAX_CONFLICT_PATH_BYTES: usize = 512;
const MAX_ERROR_CONFLICT_FILES: usize = 16;

#[derive(Debug, Clone)]
pub(super) struct MergeConflict {
    target_ref: String,
    files: Vec<String>,
    details: String,
}

pub(super) enum AttemptDisposition {
    Ready,
    Retry { task: String },
    Terminal { message: String },
}

#[derive(Debug, PartialEq, Eq)]
enum ConflictState {
    Ready,
    Aborted,
    Unresolved(Vec<String>),
}

fn classify_conflict_state(
    merge_active: bool,
    target_merged: bool,
    mut unmerged: Vec<String>,
    markers: Vec<String>,
) -> ConflictState {
    if unmerged.is_empty() && markers.is_empty() && (merge_active || target_merged) {
        return ConflictState::Ready;
    }
    if !merge_active && !target_merged {
        return ConflictState::Aborted;
    }
    unmerged.extend(markers);
    unmerged.sort();
    unmerged.dedup();
    ConflictState::Unresolved(unmerged)
}

pub(super) struct ConflictSession<'a> {
    conflict: Option<MergeConflict>,
    worktree_path: &'a Path,
    repo: &'a Path,
    session_dir: &'a Path,
    target_branch: &'a str,
}

impl<'a> ConflictSession<'a> {
    pub(super) fn prepare(
        enabled: bool,
        ledger: &mut LedgerEntry,
        worktree_path: &'a Path,
        repo: &'a Path,
        session_dir: &'a Path,
        target_branch: &'a str,
    ) -> Result<Self> {
        let conflict = if enabled {
            match refresh(ledger, worktree_path, target_branch) {
                Ok(conflict) => conflict,
                Err(error) => {
                    worktree::cleanup(worktree_path, repo);
                    return Err(error);
                }
            }
        } else {
            None
        };
        Ok(Self {
            conflict,
            worktree_path,
            repo,
            session_dir,
            target_branch,
        })
    }

    pub(super) fn is_active(&self) -> bool {
        self.conflict.is_some()
    }

    pub(super) fn append_prompt(&self, prompt: &mut String) {
        if let Some(conflict) = self.conflict.as_ref() {
            append_prompt(prompt, conflict);
        }
    }

    pub(super) fn snapshot_if_unresolved(&self, attempt: u32) -> Result<()> {
        snapshot_if_unresolved(
            self.conflict.as_ref(),
            self.worktree_path,
            self.session_dir,
            self.target_branch,
            attempt,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn after_attempt(
        &self,
        base_task: &str,
        attempt: u32,
        max_attempts: u32,
        elapsed_seconds: f64,
        ledger: &mut LedgerEntry,
        route: &RouteDecision,
        model: &str,
        result: &RunResult,
        claude_path: &str,
    ) -> Result<AttemptDisposition> {
        after_attempt(
            self.conflict.as_ref(),
            self.worktree_path,
            self.session_dir,
            self.target_branch,
            base_task,
            attempt,
            max_attempts,
            elapsed_seconds,
            ledger,
            route,
            model,
            result,
            claude_path,
        )
    }

    pub(super) fn verify_before_publish(&self, ledger: &mut LedgerEntry) -> Result<()> {
        let result = verify_before_publish(
            self.conflict.as_ref(),
            self.worktree_path,
            self.target_branch,
        );
        if let Err(error) = result {
            ledger.validation_result = Some("merge_conflict_publish_guard_failed".into());
            ledger.error_summary = Some(error.to_string());
            ledger.set_failure(FailureClass::AgentNoProgress, FailureStage::PostValidation);
            worktree::preserve_wip(
                self.worktree_path,
                self.target_branch,
                "gah: WIP conflict publish guard",
            )?;
            worktree::cleanup(self.worktree_path, self.repo);
            return Err(error);
        }
        Ok(())
    }
}

fn refresh(
    ledger: &mut LedgerEntry,
    worktree_path: &Path,
    target_branch: &str,
) -> Result<Option<MergeConflict>> {
    let outcome = classify_git_operation_result(
        ledger,
        FailureStage::Preflight,
        worktree::refresh_existing_branch_from_target(worktree_path, target_branch),
    )?;
    match outcome {
        TargetRefreshOutcome::Merged => {
            println!("Refreshed repair branch from origin/{target_branch}.");
            Ok(None)
        }
        TargetRefreshOutcome::AlreadyCurrent => {
            println!("Repair branch already contains origin/{target_branch}.");
            Ok(None)
        }
        TargetRefreshOutcome::Conflicted {
            target_ref,
            files,
            details,
        } => {
            println!(
                "Repair branch refresh has {} conflict(s); routing the live merge to the repair backend.",
                files.len()
            );
            Ok(Some(MergeConflict {
                target_ref,
                files,
                details,
            }))
        }
    }
}

fn append_prompt(prompt: &mut String, conflict: &MergeConflict) {
    prompt.push_str("\n## Unresolved Merge Conflicts\n\n");
    prompt.push_str(&format!(
        "A merge of `{}` is already in progress. Resolve it as part of this repair. \
         Preserve both accepted target-branch changes and the source ticket/review contract. \
         Do not abort, reset, or replace the merge. Remove every conflict marker and stage every \
         resolved path with `git add`. GAH will commit the completed merge only after full validation.\n\n",
        conflict.target_ref
    ));
    prompt.push_str("Conflicted files:\n");
    for file in conflict.files.iter().take(MAX_PROMPT_CONFLICT_FILES) {
        prompt.push_str("- ");
        prompt.push_str(&indent_untrusted_text(utf8_safe_prefix(
            file,
            MAX_CONFLICT_PATH_BYTES,
        )));
        prompt.push('\n');
    }
    if conflict.files.len() > MAX_PROMPT_CONFLICT_FILES {
        prompt.push_str(&format!(
            "- [truncated: {} additional conflicted files]\n",
            conflict.files.len() - MAX_PROMPT_CONFLICT_FILES
        ));
    }
    prompt.push_str("\nGit diagnostic (indented, untrusted):\n");
    prompt.push_str(&indent_untrusted_text(&conflict.details));
    prompt.push('\n');
}

fn snapshot_if_unresolved(
    conflict: Option<&MergeConflict>,
    worktree_path: &Path,
    session_dir: &Path,
    target_branch: &str,
    attempt: u32,
) -> Result<()> {
    let Some(conflict) = conflict else {
        return Ok(());
    };
    let unresolved = !worktree::unmerged_files(worktree_path)?.is_empty()
        || !worktree::files_with_conflict_markers(worktree_path, &conflict.files)?.is_empty();
    if unresolved && worktree::merge_in_progress(worktree_path)? {
        let dir = session_dir
            .join("conflict-recovery")
            .join(format!("attempt-{attempt}"));
        let path = worktree::preserve_conflict_recovery(worktree_path, &dir, target_branch)?;
        preserve_conflicted_files(worktree_path, &path, &conflict.files)?;
        println!(
            "Preserved merge-conflict recovery artifact: {}",
            path.display()
        );
    }
    Ok(())
}

fn preserve_conflicted_files(
    worktree_path: &Path,
    artifact_dir: &Path,
    files: &[String],
) -> Result<()> {
    let snapshots = artifact_dir.join("files");
    std::fs::create_dir_all(&snapshots)?;
    let mut manifest = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let relative = Path::new(file);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            continue;
        }
        let artifact = format!("{index:04}.bin");
        if let Ok(bytes) = std::fs::read(worktree_path.join(relative)) {
            std::fs::write(snapshots.join(&artifact), bytes)?;
            manifest.push(serde_json::json!({"path": file, "artifact": artifact}));
        }
    }
    std::fs::write(
        artifact_dir.join("files.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn after_attempt(
    conflict: Option<&MergeConflict>,
    worktree_path: &Path,
    session_dir: &Path,
    target_branch: &str,
    base_task: &str,
    attempt: u32,
    max_attempts: u32,
    elapsed_seconds: f64,
    ledger: &mut LedgerEntry,
    route: &RouteDecision,
    model: &str,
    result: &RunResult,
    claude_path: &str,
) -> Result<AttemptDisposition> {
    let Some(conflict) = conflict else {
        return Ok(AttemptDisposition::Ready);
    };

    let unmerged = worktree::unmerged_files(worktree_path)?;
    let markers = worktree::files_with_conflict_markers(worktree_path, &conflict.files)?;
    let merge_active = worktree::merge_in_progress(worktree_path)?;
    let target_merged = worktree::target_is_ancestor(worktree_path, target_branch)?;
    let state = classify_conflict_state(merge_active, target_merged, unmerged, markers);
    let (validation_result, message) = match state {
        ConflictState::Ready => return Ok(AttemptDisposition::Ready),
        ConflictState::Aborted => (
            "not_run_merge_conflict_aborted",
            format!(
                "repair backend removed the required merge of {}; refusing to publish a branch that does not contain the target",
                conflict.target_ref
            ),
        ),
        ConflictState::Unresolved(remaining) => (
            "not_run_unresolved_merge_conflicts",
            format!(
                "repair backend left unresolved merge conflicts in: {}",
                summarize_paths(&remaining)
            ),
        ),
    };

    ledger.attempts.push(AttemptRecord {
        attempt_number: attempt,
        backend: route.effective_backend.clone(),
        effective_model: Some(model.to_string()),
        exit_code: Some(result.exit_code),
        validation_result: Some(validation_result.into()),
        failure_class: Some(FailureClass::AgentNoProgress.as_str().into()),
        failure_stage: Some(FailureStage::AgentRun.as_str().into()),
        duration_seconds: Some(elapsed_seconds),
        diff_path: None,
        usage: attempt_usage(
            &result.log_path,
            result.agy_cli_log_delta.as_deref(),
            UsageAttribution::from_route(route).with_fallback_model(model),
            result.transcript_path.as_deref(),
            Some(claude_path),
        ),
        cli_version: result.agy_version.clone(),
    });

    snapshot_if_unresolved(
        Some(conflict),
        worktree_path,
        session_dir,
        target_branch,
        attempt,
    )?;
    if attempt < max_attempts && merge_active {
        return Ok(AttemptDisposition::Retry {
            task: format!(
                "{base_task}\n\n## Unresolved Merge Conflicts — Retry {}/{}\n\n{message}. Resolve and stage every conflicted file; do not abort the merge.",
                attempt + 1,
                max_attempts
            ),
        });
    }

    ledger.validation_result = Some(validation_result.into());
    ledger.error_summary = Some(message.clone());
    ledger.set_failure(FailureClass::AgentNoProgress, FailureStage::AgentRun);
    Ok(AttemptDisposition::Terminal { message })
}

fn verify_before_publish(
    conflict: Option<&MergeConflict>,
    worktree_path: &Path,
    target_branch: &str,
) -> Result<()> {
    let Some(conflict) = conflict else {
        return Ok(());
    };
    let unmerged = worktree::unmerged_files(worktree_path)?;
    if !unmerged.is_empty() {
        anyhow::bail!(
            "refusing to publish repair with unresolved merge conflicts: {}",
            summarize_paths(&unmerged)
        );
    }
    let markers = worktree::files_with_conflict_markers(worktree_path, &conflict.files)?;
    if !markers.is_empty() {
        anyhow::bail!(
            "refusing to publish repair with conflict markers in: {}",
            summarize_paths(&markers)
        );
    }
    if !worktree::target_is_ancestor(worktree_path, target_branch)? {
        anyhow::bail!(
            "refusing to publish repair because origin/{target_branch} is not an ancestor of HEAD"
        );
    }
    Ok(())
}

fn summarize_paths(paths: &[String]) -> String {
    let mut summary = paths
        .iter()
        .take(MAX_ERROR_CONFLICT_FILES)
        .map(|path| utf8_safe_prefix(path, MAX_CONFLICT_PATH_BYTES))
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > MAX_ERROR_CONFLICT_FILES {
        summary.push_str(&format!(
            " (+{} more)",
            paths.len() - MAX_ERROR_CONFLICT_FILES
        ));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_prompt_is_explicit_and_bounded_to_known_paths() {
        let conflict = MergeConflict {
            target_ref: "origin/main".into(),
            files: vec!["src/a.rs".into(), "src/b.rs".into()],
            details: "CONFLICT (content)".into(),
        };
        let mut prompt = String::new();
        append_prompt(&mut prompt, &conflict);
        assert!(prompt.contains("## Unresolved Merge Conflicts"));
        assert!(prompt.contains("Do not abort, reset, or replace the merge"));
        assert!(prompt.contains("src/a.rs"));
        assert!(prompt.contains("src/b.rs"));
        assert!(!prompt.contains("```"));
    }

    #[test]
    fn unchanged_conflicted_index_remains_typed_as_unresolved() {
        assert_eq!(
            classify_conflict_state(true, false, vec!["src/a.rs".into()], vec![]),
            ConflictState::Unresolved(vec!["src/a.rs".into()])
        );
    }

    #[test]
    fn staged_markers_are_not_accepted_as_resolution() {
        assert_eq!(
            classify_conflict_state(true, false, vec![], vec!["src/a.rs".into()]),
            ConflictState::Unresolved(vec!["src/a.rs".into()])
        );
    }

    #[test]
    fn aborted_merge_is_distinct_from_unresolved_conflicts() {
        assert_eq!(
            classify_conflict_state(false, false, vec![], vec![]),
            ConflictState::Aborted
        );
    }
}
