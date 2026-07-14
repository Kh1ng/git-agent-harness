use crate::config::{GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::models::CandidateArtifact;
use anyhow::Result;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::command_output;
use super::issues::parse_ticket_metadata;

/// Parallel workers: how long a "claim" entry (`LedgerEntry::new_claim`)
/// blocks a ticket before it's treated as abandoned (worker crashed/killed
/// mid-flight, or was force-killed by the idle-timeout watchdog after
/// producing partial output that never reached a real completion entry).
/// 6 hours is a generous margin above the longest real dispatch duration
/// observed in practice (~3.9h, a slow openhands/hy3 run) -- long enough
/// that a live, still-working claim is never mistaken for abandoned, short
/// enough that a genuinely dead claim doesn't block a ticket for days.
const CLAIM_STALE_AFTER_HOURS: i64 = 6;

pub(super) fn is_claim_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return true;
        }
    } else {
        return true;
    };
    let now = OffsetDateTime::now_utc();
    now - entry_time > time::Duration::hours(CLAIM_STALE_AFTER_HOURS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateWorkError {
    pub work_id: String,
    pub branch: Option<String>,
    pub mr_url: Option<String>,
}

impl fmt::Display for DuplicateWorkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Refusing dispatch: active open PR already exists for work ID '{}'",
            self.work_id
        )?;
        if let Some(url) = self.mr_url.as_deref() {
            write!(f, " ({url})")?;
        } else if let Some(branch) = self.branch.as_deref() {
            write!(f, " (branch {branch})")?;
        }
        Ok(())
    }
}

impl std::error::Error for DuplicateWorkError {}

pub(crate) fn duplicate_work_error(err: &anyhow::Error) -> Option<&DuplicateWorkError> {
    err.downcast_ref::<DuplicateWorkError>()
}

/// Parallel workers: another concurrent `gah loop`/`gah dispatch` process
/// already claimed this work_id and hasn't finished (or abandoned) it yet.
/// Distinct from `DuplicateWorkError` (which means a real PR/MR already
/// exists) since no PR/branch may exist at all here -- the other worker
/// might still be mid-backend-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveClaimError {
    pub work_id: String,
}

impl fmt::Display for ActiveClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Refusing dispatch: work ID '{}' was claimed by another in-flight dispatch within the last {CLAIM_STALE_AFTER_HOURS}h",
            self.work_id
        )
    }
}

impl std::error::Error for ActiveClaimError {}

/// Returns the resolved work_id on success (so `run()` can immediately
/// write a parallel-worker claim for it), or an error if this work_id is
/// already spoken for -- by a real open PR/MR (`DuplicateWorkError`) or by
/// another in-flight worker's claim (`ActiveClaimError`). `Ok(None)` means
/// no work_id could be resolved (nothing to claim, nothing to block).
pub(super) fn check_duplicate_work(
    cfg: &GahConfig,
    profile: &Profile,
    args: &super::DispatchArgs,
) -> Result<Option<String>> {
    let target = if args.target.is_empty() {
        if args.mode == "improve" || args.mode == "fix" {
            let default = PathBuf::from(&profile.artifact_root)
                .join("candidates")
                .join("latest.json");
            if default.exists() {
                default.to_string_lossy().into_owned()
            } else {
                args.target.clone()
            }
        } else {
            args.target.clone()
        }
    } else {
        args.target.clone()
    };

    if target.is_empty() {
        return Ok(None);
    }

    let p = Path::new(&target);
    let work_id = if p.extension().is_some_and(|e| e == "json") && p.exists() {
        if let Ok(text) = fs::read_to_string(p) {
            if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                artifact.candidates.first().map(|c| c.candidate_id.clone())
            } else {
                None
            }
        } else {
            None
        }
    } else if p.extension().is_some_and(|e| e == "md") && p.exists() {
        if let Ok(Some(ticket)) = parse_ticket_metadata(p) {
            ticket.work_id.clone().or(ticket.ticket_id.clone())
        } else {
            None
        }
    } else {
        None
    };

    let Some(work_id) = work_id else {
        return Ok(None);
    };

    let matching_entries = match crate::ledger::entries_for_work_id(cfg, &work_id) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("warning: failed to read ledger entries: {:#}", e);
            return Ok(Some(work_id));
        }
    };

    if matching_entries.is_empty() {
        return Ok(Some(work_id));
    }

    // Try to fetch MRs/PRs from provider
    let mrs = crate::sync::fetch_mrs(profile).unwrap_or_default();

    for entry in matching_entries {
        if super::is_ledger_entry_stale(&entry) {
            continue;
        }

        // Parallel workers: another concurrent dispatch already claimed
        // this work_id and hasn't finished (or been abandoned long enough
        // to ignore) yet.
        if entry.mode == "claim" && !is_claim_stale(&entry) {
            return Err(anyhow::Error::new(ActiveClaimError {
                work_id: work_id.clone(),
            }));
        }

        // Check if there is a matching MR
        let matching_mr = mrs.iter().find(|mr| {
            if let Some(ref entry_branch) = entry.branch {
                if mr.branch == *entry_branch {
                    return true;
                }
            }
            if let Some(ref entry_mr_url) = entry.mr_url {
                if mr.url.as_ref() == Some(entry_mr_url) {
                    return true;
                }
            }
            false
        });

        if let Some(mr) = matching_mr {
            let class = crate::sync::classify(mr);
            if class == "MERGED" {
                continue;
            }
            if class == "CLOSED_UNMERGED" {
                continue;
            }
            if class == "STALE" {
                continue;
            }
            // Otherwise, it's an active open PR -> Block
            return Err(anyhow::Error::new(DuplicateWorkError {
                work_id: work_id.clone(),
                branch: Some(mr.branch.clone()),
                mr_url: mr.url.clone(),
            }));
        }

        // If no matching MR is found, check if branch exists
        let repo_path = Path::new(&profile.local_path);
        if let Some(ref branch_name) = entry.branch {
            if command_output("git", &["rev-parse", "--verify", branch_name], repo_path).is_ok() {
                println!(
                    "Warning: active branch '{}' may already own work for work ID '{}'",
                    branch_name, work_id
                );
            }
        }
    }

    Ok(Some(work_id))
}

#[cfg(test)]
mod tests {
    use super::check_duplicate_work;
    use super::duplicate_work_error;
    use super::DuplicateWorkError;
    use crate::config::{GahConfig, Profile, RoutingPolicy};
    use crate::ledger::LedgerEntry;
    use crate::test_support::PathGuard;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use time::OffsetDateTime;

    fn profile(local_path: &Path) -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            prune_older_than_days: None,
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: local_path.display().to_string(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
            publishing: Default::default(),
        }
    }

    fn setup_fake_gh(bin_dir: &Path, response_json: &str) {
        let gh_path = bin_dir.join("gh");
        let content = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then\n\
                 echo '{}'\n\
             fi\n",
            response_json.replace('\'', "'\\''")
        );
        fs::write(&gh_path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
    }

    fn init_repo(repo: &Path) {
        fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo)
            .output()
            .unwrap();
        fs::write(repo.join("README.md"), "hi\n").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    #[test]
    fn duplicate_work_error_detection_is_typed_not_string_matched() {
        let err = anyhow::Error::new(DuplicateWorkError {
            work_id: "TICKET-999".into(),
            branch: Some("gah/repo-999".into()),
            mr_url: Some("https://example/pull/999".into()),
        })
        .context("outer wording changed completely");

        let duplicate = duplicate_work_error(&err).unwrap();
        assert_eq!(duplicate.work_id, "TICKET-999");
        assert_eq!(duplicate.branch.as_deref(), Some("gah/repo-999"));
        assert_eq!(
            duplicate.mr_url.as_deref(),
            Some("https://example/pull/999")
        );
    }

    #[test]
    fn test_check_duplicate_work_cases() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        let ticket_path = ticket_dir.join("TICKET-097-test.md");
        fs::write(
            &ticket_path,
            "# TICKET-097: Test ticket\n\n\
             Goal: Test duplicate work guard\n\n\
             ## Problem\n\
             Test\n",
        )
        .unwrap();

        let cfg = GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };

        let mut prof = profile(tmp.path());
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let ledger_path = tmp.path().join("ledger.jsonl");

        let args = super::super::DispatchArgs {
            profile: "test".to_string(),
            mode: "improve".to_string(),
            backend: "codex".to_string(),
            target: ticket_path.display().to_string(),
            branch: None,
            mr: None,
            current_branch: false,
            budget: 0,
            dry_run: false,
            config_path: None,
            oh_profile: None,
            model: None,
            retries: 0,
            allow_draft_fail: false,
            prod: false,
            allow_unknown_red_baseline: false,
            escalate: false,
            existing_branch: None,
            skip_validation_gate: false,
            dispatch_reason: None,
            work_id: None,
            run_id: None,
            route_ready: None,
        };

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        let pr_json = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"OPEN","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":null,"updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json);
        let _guard = PathGuard::set(&bin_dir);

        let mut entry = LedgerEntry::new(
            "test",
            &prof,
            "codex",
            "improve",
            &ticket_path.display().to_string(),
            Some("session-1".into()),
            None,
        );
        entry.work_id = Some("TICKET-097".to_string());
        entry.branch = Some("gah/repo-active".to_string());
        entry.mr_url = Some("https://github.com/owner/repo/pull/1".to_string());
        entry.timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();

        let ledger_line = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line)).unwrap();

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_err());
        let err = res.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Refusing dispatch: active open PR already exists"));
        let duplicate = duplicate_work_error(&err).unwrap();
        assert_eq!(duplicate.work_id, "TICKET-097");
        assert_eq!(
            duplicate.mr_url.as_deref(),
            Some("https://github.com/owner/repo/pull/1")
        );

        let pr_json_merged = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"MERGED","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":"2026-07-04T17:22:35-05:00","updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json_merged);

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        let pr_json_closed = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"CLOSED","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":null,"updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json_closed);

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        setup_fake_gh(&bin_dir, pr_json);
        entry.timestamp = (OffsetDateTime::now_utc() - time::Duration::days(15))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let ledger_line_stale = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line_stale)).unwrap();

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        setup_fake_gh(&bin_dir, "[]");
        let local_repo_path = tmp.path().join("local_repo");
        fs::create_dir_all(&local_repo_path).unwrap();
        init_repo(&local_repo_path);
        Command::new("git")
            .args(["branch", "gah/repo-active"])
            .current_dir(&local_repo_path)
            .output()
            .unwrap();
        let mut prof_with_repo = prof.clone();
        prof_with_repo.local_path = local_repo_path.display().to_string();

        entry.timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let ledger_line_active_branch = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line_active_branch)).unwrap();

        let res = check_duplicate_work(&cfg, &prof_with_repo, &args);
        assert!(res.is_ok());
    }

    #[test]
    fn check_duplicate_work_blocks_on_active_claim() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        setup_fake_gh(&bin_dir, "[]");
        let _guard = PathGuard::set(&bin_dir);

        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        let ticket_path = ticket_dir.join("TICKET-500-test.md");
        fs::write(
            &ticket_path,
            "# TICKET-500: Test\n\nGoal: test claim guard\n",
        )
        .unwrap();

        let cfg = GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let ledger_path = tmp.path().join("ledger.jsonl");
        let claim = LedgerEntry::new_claim("test", &prof, "TICKET-500");
        fs::write(
            &ledger_path,
            format!("{}\n", serde_json::to_string(&claim).unwrap()),
        )
        .unwrap();

        let args = super::super::DispatchArgs {
            profile: "test".to_string(),
            mode: "improve".to_string(),
            backend: "codex".to_string(),
            target: ticket_path.display().to_string(),
            branch: None,
            mr: None,
            current_branch: false,
            budget: 0,
            dry_run: false,
            config_path: None,
            oh_profile: None,
            model: None,
            retries: 0,
            allow_draft_fail: false,
            prod: false,
            allow_unknown_red_baseline: false,
            escalate: false,
            existing_branch: None,
            skip_validation_gate: false,
            dispatch_reason: None,
            work_id: None,
            run_id: None,
            route_ready: None,
        };

        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("claimed by another in-flight dispatch"));

        let mut stale_claim = claim.clone();
        stale_claim.timestamp = (OffsetDateTime::now_utc() - time::Duration::hours(7))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        fs::write(
            &ledger_path,
            format!("{}\n", serde_json::to_string(&stale_claim).unwrap()),
        )
        .unwrap();
        let res = check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());
    }
}
