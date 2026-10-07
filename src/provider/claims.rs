//! GitHub assignees coordinate installations; local dispatch claims stay separate.
use super::{github_find_pr_number_by_branch, github_json_api};
use crate::config::Profile;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

thread_local! {
    static CLAIM_LOST: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub(crate) fn claim_lost() -> bool {
    CLAIM_LOST.with(|flag| {
        flag.borrow()
            .as_ref()
            .is_some_and(|f| f.load(Ordering::SeqCst))
    })
}

#[derive(Debug)]
struct Observation {
    assignees: Vec<String>,
    activity: DateTime<Utc>,
    open: bool,
}

impl Observation {
    fn owned_by(&self, identity: &str) -> bool {
        self.open && self.assignees.len() == 1 && self.assignees[0] == identity
    }
    fn reclaimable(&self, now: DateTime<Utc>, lease_seconds: u64) -> bool {
        self.open
            && (self.assignees.is_empty()
                || now.signed_duration_since(self.activity).num_seconds()
                    > i64::try_from(lease_seconds).unwrap_or(i64::MAX))
    }
}

// Provider operations are injected in tests so races are measured by ownership,
// not by the controller's scheduling or local claim-store implementation.
fn settle_claim(
    identity: &str,
    mut read: impl FnMut() -> Result<Observation>,
    assign: impl FnOnce() -> Result<()>,
    pause: impl FnOnce(),
    lease_seconds: u64,
) -> Result<bool> {
    if !read()?.reclaimable(Utc::now(), lease_seconds) {
        return Ok(false);
    }
    assign()?;
    pause();
    Ok(read()?.owned_by(identity))
}

fn observe(profile: &Profile, number: &str, pr: bool) -> Result<Observation> {
    let value = github_json_api(
        profile,
        "GET",
        &format!("repos/{}/issues/{number}", profile.repo),
        &[],
    )?;
    let mut activity = value["updated_at"]
        .as_str()
        .context("missing claim activity")?
        .parse::<DateTime<Utc>>()?;
    if pr {
        // A push and a check/status transition need not update the issue timestamp.
        let pull = github_json_api(
            profile,
            "GET",
            &format!("repos/{}/pulls/{number}", profile.repo),
            &[],
        )?;
        if let Some(date) = pull["updated_at"].as_str() {
            activity = activity.max(date.parse()?);
        }
        let sha = pull["head"]["sha"]
            .as_str()
            .context("missing claimed PR head")?;
        let commit = github_json_api(
            profile,
            "GET",
            &format!("repos/{}/commits/{sha}", profile.repo),
            &[],
        )?;
        if let Some(date) = commit["commit"]["committer"]["date"].as_str() {
            activity = activity.max(date.parse()?);
        }
        for suffix in ["status", "check-runs"] {
            let statuses = github_json_api(
                profile,
                "GET",
                &format!("repos/{}/commits/{sha}/{suffix}", profile.repo),
                &[],
            )?;
            let rows = statuses["statuses"]
                .as_array()
                .or_else(|| statuses["check_runs"].as_array());
            if let Some(rows) = rows {
                for row in rows {
                    for key in ["updated_at", "started_at", "completed_at"] {
                        if let Some(date) = row[key].as_str() {
                            activity = activity.max(date.parse()?);
                        }
                    }
                }
            }
        }
    }
    let assignees = value["assignees"]
        .as_array()
        .context("missing claim assignees")?
        .iter()
        .map(|a| {
            a["login"]
                .as_str()
                .map(str::to_owned)
                .context("missing assignee login")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Observation {
        assignees,
        activity,
        open: value["state"].as_str() == Some("open"),
    })
}

/// Holds one provider claim for the entire dispatch, including validation and publication.
pub(crate) struct GithubClaim {
    stop: std::sync::mpsc::Sender<()>,
    monitor: Option<JoinHandle<()>>,
    lost: Arc<AtomicBool>,
    profile: Profile,
    number: String,
    pr: bool,
    identity: String,
}

impl GithubClaim {
    pub(crate) fn acquire(
        profile: &Profile,
        work_id: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Option<Self>> {
        let identity = profile
            .publishing
            .github_claim_identity
            .as_deref()
            .context("missing GitHub claim identity")?;
        anyhow::ensure!(
            !identity.trim().is_empty(),
            "GitHub claim identity must not be empty"
        );
        anyhow::ensure!(
            profile.publishing.github_claim_settle_seconds > 0
                && profile.publishing.github_claim_lease_seconds > 0
                && profile.publishing.github_claim_poll_seconds > 0,
            "GitHub claim timing must be positive"
        );
        let pr = branch.is_some();
        let number = match branch {
            Some(branch) => github_find_pr_number_by_branch(profile, branch)?,
            None => {
                let key = crate::work_claim::normalize_work_identity(
                    work_id.context("provider claim requires issue identity")?,
                );
                key.strip_prefix('#')
                    .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    .context("provider claim requires numeric GitHub issue identity")?
                    .to_string()
            }
        };
        let won = settle_claim(
            identity,
            || observe(profile, &number, pr),
            || {
                github_json_api(
                    profile,
                    "PATCH",
                    &format!("repos/{}/issues/{number}", profile.repo),
                    &[("assignees[]", identity)],
                )?;
                Ok(())
            },
            || {
                thread::sleep(Duration::from_secs(
                    profile.publishing.github_claim_settle_seconds,
                ))
            },
            profile.publishing.github_claim_lease_seconds,
        )?;
        if !won {
            return Ok(None);
        }
        let lost = Arc::new(AtomicBool::new(false));
        CLAIM_LOST.with(|flag| *flag.borrow_mut() = Some(lost.clone()));
        let (stop, rx) = std::sync::mpsc::channel();
        let monitor_profile = profile.clone();
        let monitor_number = number.clone();
        let monitor_identity = identity.to_owned();
        let monitor_lost = lost.clone();
        #[cfg(test)]
        let provider_path = super::TEST_PATH_OVERRIDE.with(|path| path.borrow().clone());
        let monitor = thread::spawn(move || {
            #[cfg(test)]
            super::TEST_PATH_OVERRIDE.with(|path| *path.borrow_mut() = provider_path);
            while rx
                .recv_timeout(Duration::from_secs(
                    monitor_profile.publishing.github_claim_poll_seconds,
                ))
                .is_err()
            {
                match observe(&monitor_profile, &monitor_number, pr) {
                    Ok(item) if item.owned_by(&monitor_identity) => {}
                    _ => {
                        // Unavailable ownership is a stop condition, never presumed ownership.
                        monitor_lost.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });
        Ok(Some(Self {
            stop,
            monitor: Some(monitor),
            lost,
            profile: profile.clone(),
            number,
            pr,
            identity: identity.to_owned(),
        }))
    }
    pub(crate) fn ensure_owned(&self) -> Result<()> {
        anyhow::ensure!(
            !self.lost.load(Ordering::SeqCst)
                && observe(&self.profile, &self.number, self.pr)?.owned_by(&self.identity),
            "GitHub claim lost; worker stopped"
        );
        Ok(())
    }
}

impl Drop for GithubClaim {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
        CLAIM_LOST.with(|flag| *flag.borrow_mut() = None);
        // Remove only this bot; never clear a successor's assignment.
        let _ = github_json_api(
            &self.profile,
            "DELETE",
            &format!(
                "repos/{}/issues/{}/assignees",
                self.profile.repo, self.number
            ),
            &[("assignees[]", &self.identity)],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settle_race_has_one_winner_and_loser_can_claim_next_item() {
        use std::cell::Cell;
        let owner = RefCell::new(Vec::<String>::new());
        let other_won = Cell::new(false);
        let read = || {
            Ok(Observation {
                assignees: owner.borrow().clone(),
                activity: Utc::now(),
                open: true,
            })
        };
        let won = settle_claim(
            "bot-a",
            read,
            || {
                *owner.borrow_mut() = vec!["bot-a".into()];
                Ok(())
            },
            || {
                // Bot B's initial read happened before A assigned itself.
                // Its write arrives during A's mandatory settle pause.
                let initial_read = Cell::new(true);
                other_won.set(
                    settle_claim(
                        "bot-b",
                        || {
                            if initial_read.replace(false) {
                                Ok(Observation {
                                    assignees: vec![],
                                    activity: Utc::now(),
                                    open: true,
                                })
                            } else {
                                read()
                            }
                        },
                        || {
                            *owner.borrow_mut() = vec!["bot-b".into()];
                            Ok(())
                        },
                        || {},
                        900,
                    )
                    .unwrap(),
                );
            },
            900,
        )
        .unwrap();
        assert!(!won);
        assert!(other_won.get());
        assert_eq!(*owner.borrow(), vec!["bot-b"]);
        owner.borrow_mut().clear();
        assert!(settle_claim(
            "bot-a",
            read,
            || {
                *owner.borrow_mut() = vec!["bot-a".into()];
                Ok(())
            },
            || {},
            900
        )
        .unwrap());
    }

    #[test]
    fn losing_ownership_stops_only_the_claiming_worker() {
        let flag = Arc::new(AtomicBool::new(false));
        CLAIM_LOST.with(|slot| *slot.borrow_mut() = Some(flag.clone()));
        assert!(!claim_lost());
        flag.store(true, Ordering::SeqCst);
        assert!(claim_lost());
        assert!(crate::runner::shutdown_requested());
        assert!(!thread::spawn(claim_lost).join().unwrap());
        CLAIM_LOST.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn lost_claim_terminates_running_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        CLAIM_LOST.with(|slot| *slot.borrow_mut() = Some(flag.clone()));
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::SeqCst);
        });
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 60"]);
        let started = std::time::Instant::now();
        let result = crate::runner::process::spawn_with_idle_watch(
            command,
            &tmp.path().join("worker.log"),
            tmp.path(),
            60,
            "claim-loss test",
        );
        setter.join().unwrap();
        CLAIM_LOST.with(|slot| *slot.borrow_mut() = None);
        assert_eq!(result.unwrap().0, -2);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn two_factories_drain_one_mixed_queue_without_double_claims() {
        use std::collections::BTreeMap;
        // Issues need implement; PRs need review or fix. No item belongs to a fleet.
        let queue = [
            "issue:1", "pr:2", "issue:3", "pr:4", "pr:5", "issue:6", "pr:7",
        ];
        let owners = RefCell::new(BTreeMap::<&str, Vec<String>>::new());
        let mut done = BTreeMap::<&str, String>::new();
        let factories = [("fleet-a", 2usize), ("fleet-b", 3usize)];
        while done.len() < queue.len() {
            for (identity, parallelism) in factories {
                let mut started = 0;
                for item in queue {
                    if started == parallelism {
                        break;
                    }
                    if done.contains_key(item) {
                        continue;
                    }
                    let read = || {
                        Ok(Observation {
                            assignees: owners.borrow().get(item).cloned().unwrap_or_default(),
                            activity: Utc::now(),
                            open: true,
                        })
                    };
                    let assign = || {
                        owners.borrow_mut().insert(item, vec![identity.into()]);
                        Ok(())
                    };
                    if settle_claim(identity, read, assign, || {}, 900).unwrap() {
                        started += 1;
                        assert!(done.insert(item, identity.into()).is_none());
                    }
                }
                assert!(started <= parallelism);
            }
        }
        assert_eq!(done.len(), queue.len());
        assert!(done.values().any(|owner| owner == "fleet-a"));
        assert!(done.values().any(|owner| owner == "fleet-b"));
        for item in queue {
            assert_eq!(owners.borrow()[item], vec![done[item].clone()]);
        }
    }

    #[test]
    fn lease_protects_progress_and_unknown_ownership_is_not_owned() {
        let now = Utc::now();
        let mut item = Observation {
            assignees: vec!["bot-a".into()],
            activity: now - chrono::Duration::seconds(901),
            open: true,
        };
        assert!(item.reclaimable(now, 900));
        item.activity = now - chrono::Duration::seconds(899);
        assert!(!item.reclaimable(now, 900));
        assert!(item.owned_by("bot-a"));
        assert!(!item.owned_by("bot-b"));
        item.assignees.push("bot-b".into());
        assert!(!item.owned_by("bot-a"));
        item.open = false;
        assert!(!item.reclaimable(now, 900));
    }
}
