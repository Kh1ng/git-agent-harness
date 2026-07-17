use super::super::issues::{parse_ticket_metadata, TicketMetadata};
use super::super::text::normalize_match;
use super::super::DispatchArgs;
use crate::config::{GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::provider;
use crate::worktree;
use anyhow::Result;
use std::path::Path;

pub(in crate::dispatch) fn resolve_review_target(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
) -> Result<ReviewTarget> {
    if let Some(mr) = args.mr.as_deref() {
        let mr_target = provider::find_review_target_by_mr(profile, mr)?;
        let metadata_fingerprint = crate::sync::review_metadata_fingerprint(
            mr_target.source_sha.as_deref(),
            mr_target.title.as_deref(),
            mr_target.body.as_deref(),
            mr_target.draft,
        );
        return Ok(ReviewTarget {
            mr_id: Some(mr_target.id),
            mr_url: Some(mr_target.url),
            mr_title: mr_target.title,
            mr_body: mr_target.body,
            ci_status: mr_target.ci_status,
            source_sha: mr_target.source_sha,
            metadata_fingerprint: Some(metadata_fingerprint),
            draft: mr_target.draft,
            target_sha: mr_target.target_sha,
            source_branch: mr_target.source_branch.clone(),
            target_branch: fallback_target_branch(
                &profile.default_target_branch,
                Some(&mr_target.target_branch),
            ),
            prior_state: lookup_review_state_by_branch(
                cfg,
                &args.profile,
                &mr_target.source_branch,
            ),
        });
    }

    if let Some(branch) = args.branch.as_deref() {
        return review_target_from_branch(profile, branch);
    }

    if !args.target.is_empty() {
        let target_path = Path::new(&args.target);
        if let Some(ticket) = parse_ticket_metadata(target_path)? {
            if let Some(state) =
                lookup_review_state(cfg, profile, &args.profile, &args.target, &ticket)
            {
                return Ok(state);
            }
        } else {
            return review_target_from_branch(profile, &args.target);
        }
    }

    if args.current_branch {
        let repo = Path::new(&profile.local_path);
        let branch = worktree::git(&["rev-parse", "--abbrev-ref", "HEAD"], repo)?;
        return review_target_from_branch(profile, &branch);
    }

    anyhow::bail!(
        "review target required: pass --mr, --branch, a ticket path in --target, or --current-branch"
    )
}

pub(in crate::dispatch) fn review_target_from_branch(
    profile: &Profile,
    branch: &str,
) -> Result<ReviewTarget> {
    match provider::find_review_target_by_branch(profile, branch) {
        Ok(mr_target) => {
            let metadata_fingerprint = crate::sync::review_metadata_fingerprint(
                mr_target.source_sha.as_deref(),
                mr_target.title.as_deref(),
                mr_target.body.as_deref(),
                mr_target.draft,
            );
            Ok(ReviewTarget {
                mr_id: Some(mr_target.id),
                mr_url: Some(mr_target.url),
                source_branch: if mr_target.source_branch.is_empty() {
                    branch.to_string()
                } else {
                    mr_target.source_branch
                },
                target_branch: fallback_target_branch(
                    &profile.default_target_branch,
                    Some(&mr_target.target_branch),
                ),
                mr_title: mr_target.title,
                mr_body: mr_target.body,
                ci_status: mr_target.ci_status,
                source_sha: mr_target.source_sha,
                metadata_fingerprint: Some(metadata_fingerprint),
                draft: mr_target.draft,
                target_sha: mr_target.target_sha,
                prior_state: None,
            })
        }
        Err(_) => Ok(ReviewTarget {
            mr_id: None,
            mr_url: None,
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_sha: None,
            metadata_fingerprint: None,
            draft: false,
            target_sha: None,
            source_branch: branch.to_string(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: None,
        }),
    }
}

pub(in crate::dispatch) fn fallback_target_branch(
    default_branch: &str,
    provider_target: Option<&str>,
) -> String {
    provider_target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_branch)
        .to_string()
}

pub(in crate::dispatch) fn lookup_review_state(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    target: &str,
    ticket: &TicketMetadata,
) -> Option<ReviewTarget> {
    let entries = ledger::read_entries(cfg).ok()?;
    let ticket_id = ticket.ticket_id.as_deref();
    let ticket_title = ticket.title.as_deref().map(normalize_match);
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.is_some()
                && entry.error_summary.is_none()
                && (entry.target_summary.as_deref() == Some(target)
                    || ticket_id
                        .map(|id| entry.target_summary.as_deref().unwrap_or("").contains(id))
                        .unwrap_or(false)
                    || ticket_title
                        .as_ref()
                        .map(|title| {
                            normalize_match(entry.target_summary.as_deref().unwrap_or(""))
                                .contains(title)
                        })
                        .unwrap_or(false))
        })
        .map(|entry| ReviewTarget {
            mr_id: entry
                .mr_url
                .as_deref()
                .and_then(|url| url.rsplit('/').next())
                .map(str::to_string),
            mr_url: entry.mr_url.clone(),
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_sha: None,
            metadata_fingerprint: None,
            draft: false,
            target_sha: None,
            source_branch: entry.branch.clone().unwrap_or_default(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: Some(render_prior_ledger_state(&entry)),
        })
}

pub(in crate::dispatch) fn lookup_review_state_by_branch(
    cfg: &GahConfig,
    profile_name: &str,
    branch: &str,
) -> Option<String> {
    let entries = ledger::read_entries(cfg).ok()?;
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.as_deref() == Some(branch)
        })
        .map(|entry| render_prior_ledger_state(&entry))
}

pub(in crate::dispatch) fn render_prior_ledger_state(entry: &LedgerEntry) -> String {
    format!(
        "Mode: {}\nRequested backend/model: {} / {}\nEffective backend/model: {} / {}\nValidation result: {}\nMR: {}\nSession: {}",
        entry.mode,
        entry.requested_backend,
        entry.requested_model.as_deref().unwrap_or("unknown"),
        entry.effective_backend,
        entry.effective_model.as_deref().unwrap_or("unknown"),
        entry.validation_result.as_deref().unwrap_or("unknown"),
        entry.mr_url.as_deref().unwrap_or("n/a"),
        entry.session_dir.as_deref().unwrap_or("n/a"),
    )
}

pub(in crate::dispatch) fn prepare_review_diff(
    repo: &Path,
    _profile: &Profile,
    target: &mut ReviewTarget,
) -> Result<ReviewDiffBundle> {
    worktree::git(&["fetch", "-q", "origin", "--prune"], repo)?;
    worktree::git(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.target_branch, target.target_branch
            ),
        ],
        repo,
    )?;
    worktree::git(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.source_branch, target.source_branch
            ),
        ],
        repo,
    )?;

    let target_ref = format!("origin/{}", target.target_branch);
    let source_ref = format!("origin/{}", target.source_branch);
    capture_review_ref_shas(repo, target, &target_ref, &source_ref)?;
    let diff = worktree::git(&["diff", &format!("{target_ref}...{source_ref}")], repo)?;
    let files = worktree::git(
        &[
            "diff",
            "--name-only",
            &format!("{target_ref}...{source_ref}"),
        ],
        repo,
    )?;
    if diff.trim().is_empty() {
        anyhow::bail!(empty_review_diff_diagnostics(
            repo,
            target,
            &target_ref,
            &source_ref
        ));
    }
    Ok(ReviewDiffBundle { diff, files })
}

fn capture_review_ref_shas(
    repo: &Path,
    target: &mut ReviewTarget,
    target_ref: &str,
    source_ref: &str,
) -> Result<()> {
    // Record the immutable commits from the exact refs used to construct the
    // diff. Provider metadata is useful enrichment, but it must not be the
    // authority here: provider CLIs expose different SHA fields (and older
    // `gh` versions do not expose a base-ref OID at all). FixMr relies on the
    // source SHA to prove that review findings still apply to the branch.
    target.target_sha = Some(worktree::git(&["rev-parse", target_ref], repo)?);
    target.source_sha = Some(worktree::git(&["rev-parse", source_ref], repo)?);
    Ok(())
}

pub(in crate::dispatch) fn empty_review_diff_diagnostics(
    repo: &Path,
    target: &ReviewTarget,
    target_ref: &str,
    source_ref: &str,
) -> String {
    let current_branch = worktree::git(&["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(error: {e:#})"));
    let target_sha = worktree::git(&["rev-parse", target_ref], repo)
        .unwrap_or_else(|e| format!("(error: {e:#})"));
    let source_sha = worktree::git(&["rev-parse", source_ref], repo)
        .unwrap_or_else(|e| format!("(error: {e:#})"));
    let diff_stat = worktree::git(
        &["diff", "--stat", &format!("{target_ref}...{source_ref}")],
        repo,
    )
    .unwrap_or_else(|e| format!("(error: {e:#})"));
    format!(
        "empty review diff\nprofile.local_path: {}\ncurrent branch: {}\nsource branch: {}\ntarget branch: {}\nfetched refs: {}, {}\ngit rev-parse target: {}\ngit rev-parse source: {}\ngit diff --stat:\n{}\nsuggestion: fetch the source branch or pass --branch/--mr for the open review target explicitly",
        repo.display(),
        current_branch,
        target.source_branch,
        target.target_branch,
        source_ref,
        target_ref,
        target_sha,
        source_sha,
        diff_stat,
    )
}

#[derive(Debug, Clone)]
pub(in crate::dispatch) struct ReviewTarget {
    pub(in crate::dispatch) mr_id: Option<String>,
    pub(in crate::dispatch) mr_url: Option<String>,
    pub(in crate::dispatch) mr_title: Option<String>,
    pub(in crate::dispatch) mr_body: Option<String>,
    pub(in crate::dispatch) ci_status: Option<String>,
    pub(in crate::dispatch) source_sha: Option<String>,
    pub(in crate::dispatch) metadata_fingerprint: Option<String>,
    pub(in crate::dispatch) draft: bool,
    pub(in crate::dispatch) target_sha: Option<String>,
    pub(in crate::dispatch) source_branch: String,
    pub(in crate::dispatch) target_branch: String,
    pub(in crate::dispatch) prior_state: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::dispatch) struct ReviewDiffBundle {
    pub(in crate::dispatch) diff: String,
    pub(in crate::dispatch) files: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn review_shas_come_from_the_exact_fetched_diff_refs() {
        let tmp = tempfile::tempdir().unwrap();
        crate::dispatch::test_util::init_repo(tmp.path());
        let base = worktree::git(&["rev-parse", "HEAD"], tmp.path()).unwrap();
        Command::new("git")
            .args(["update-ref", "refs/remotes/origin/main", &base])
            .current_dir(tmp.path())
            .status()
            .unwrap();

        std::fs::write(tmp.path().join("change.txt"), "review me\n").unwrap();
        Command::new("git")
            .args(["add", "change.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "change"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let source = worktree::git(&["rev-parse", "HEAD"], tmp.path()).unwrap();
        Command::new("git")
            .args(["update-ref", "refs/remotes/origin/feature", &source])
            .current_dir(tmp.path())
            .status()
            .unwrap();

        let mut target = ReviewTarget {
            mr_id: None,
            mr_url: None,
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_sha: None,
            metadata_fingerprint: None,
            draft: false,
            target_sha: None,
            source_branch: "feature".to_string(),
            target_branch: "main".to_string(),
            prior_state: None,
        };
        capture_review_ref_shas(tmp.path(), &mut target, "origin/main", "origin/feature").unwrap();

        assert_eq!(target.target_sha.as_deref(), Some(base.as_str()));
        assert_eq!(target.source_sha.as_deref(), Some(source.as_str()));
    }
}
