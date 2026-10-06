//! Read-only projection of dispatch invocation records in session artifacts.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Clone, Serialize, Deserialize)]
pub struct RunningWorker {
    pub profile: String,
    pub work_id: Option<String>,
    pub run_id: String,
    pub mode: String,
    pub backend: String,
    pub runner: String,
    pub backend_instance: String,
    pub requested_model: Option<String>,
    /// Routed model; actual model remains unknown until reported by the backend.
    pub model: Option<String>,
    pub actual_model: Option<String>,
    pub node_id: Option<String>,
    pub branch: Option<String>,
    pub started_at: String,
    pub attempt: u32,
    pub last_activity_at: String,
    pub stale_after_seconds: u64,
    pub state: String,
}

#[derive(Serialize, Deserialize)]
pub struct DispatchContext {
    pub profile: String,
    pub work_id: Option<String>,
    pub run_id: String,
    pub mode: String,
}

// Ownership belongs to the artifact, not the public roster contract. Old records
// without ownership remain readable. All records expire after a day without activity.
#[derive(Serialize, Deserialize)]
struct InvocationRecord {
    #[serde(flatten)]
    worker: RunningWorker,
    #[serde(default)]
    owner_pid: Option<u32>,
}

fn owner_is_gone(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return true;
        };
        if pid <= 0 {
            return true;
        }
        // Signal zero probes existence without signalling the dispatch. EPERM
        // means the process exists; only ESRCH establishes that it is gone.
        (unsafe { libc::kill(pid, 0) == -1 })
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Renew only when the runner's idle watcher observes actual progress.
/// Missing or unwritable observation artifacts must never affect execution.
pub(crate) fn heartbeat(session: &Path) {
    if let Ok(file) = fs::OpenOptions::new()
        .write(true)
        .open(session.join("running-worker.json"))
    {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

pub fn initialize(session: &Path, context: &DispatchContext) {
    if let Ok(bytes) = serde_json::to_vec(context) {
        let _ = fs::write(session.join("dispatch-context.json"), bytes);
    }
}

/// Observation failures must not change dispatch behavior. On normal completion
/// Drop removes the record; observation marks records left by dead owners stale.
pub struct InvocationGuard(Option<PathBuf>);
impl InvocationGuard {
    pub fn start(
        session: &Path,
        identity: &crate::execution_identity::ExecutionIdentity,
        branch: Option<String>,
        work_id: Option<&str>,
        stale_after_seconds: u64,
    ) -> Self {
        let context = session.ancestors().find_map(|dir| {
            fs::read(dir.join("dispatch-context.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<DispatchContext>(&bytes).ok())
        });
        let Some(context) = context else {
            return Self(None);
        };
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default();
        let attempt = session
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('-').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let worker = RunningWorker {
            profile: context.profile,
            work_id: work_id.map(str::to_string).or(context.work_id),
            run_id: context.run_id,
            mode: context.mode,
            backend: identity.logical_backend.clone(),
            runner: identity.runner_kind.as_str().to_string(),
            backend_instance: identity.backend_instance.clone(),
            requested_model: identity.requested_model.clone(),
            model: identity.effective_model.clone(),
            actual_model: None,
            node_id: crate::central_claims::resolve_node_id().ok(),
            branch,
            started_at: now.clone(),
            attempt,
            last_activity_at: now,
            stale_after_seconds,
            state: "running".into(),
        };
        let path = session.join("running-worker.json");
        let temporary = path.with_extension("json.tmp");
        let written = serde_json::to_vec(&InvocationRecord {
            worker,
            owner_pid: Some(std::process::id()),
        })
        .ok()
        .and_then(|bytes| {
            fs::write(&temporary, bytes).ok()?;
            fs::rename(&temporary, &path).ok()
        });
        Self(written.map(|_| path))
    }
}
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn observe(root: &Path, now: OffsetDateTime) -> Vec<RunningWorker> {
    fn visit(dir: &Path, now: OffsetDateTime, rows: &mut Vec<RunningWorker>, depth: usize) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                visit(&path, now, rows, depth + 1);
            } else if entry.file_name() == "running-worker.json" {
                let Some(record) = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<InvocationRecord>(&bytes).ok())
                else {
                    continue;
                };
                let owner_gone = record.owner_pid.is_some_and(owner_is_gone);
                let mut row = record.worker;
                let mut last = OffsetDateTime::parse(&row.started_at, &Rfc3339).unwrap_or(now);
                if let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) {
                    last = last.max(OffsetDateTime::from(modified));
                }
                // Output and idle-watch heartbeats are activity; reads never renew it.
                if let Ok(logs) = fs::read_dir(dir) {
                    for log in logs
                        .flatten()
                        .filter(|log| log.file_name().to_string_lossy().ends_with(".log"))
                    {
                        if let Ok(modified) = log.metadata().and_then(|m| m.modified()) {
                            last = last.max(OffsetDateTime::from(modified));
                        }
                    }
                }
                // Also bound legacy records, PID reuse after reboot, and platforms
                // without a PID probe. Healthy workers renew through the idle watch.
                if (now - last).whole_seconds() >= 86_400 {
                    continue;
                }
                row.last_activity_at = last.format(&Rfc3339).unwrap_or_default();
                row.state = if owner_gone
                    || (now - last).whole_seconds() >= row.stale_after_seconds as i64
                {
                    "stale"
                } else {
                    "running"
                }
                .into();
                rows.push(row);
            }
        }
    }
    let mut rows = Vec::new();
    visit(&root.join("sessions"), now, &mut rows, 0);
    rows.sort_by(|a, b| (&a.run_id, a.attempt).cmp(&(&b.run_id, b.attempt)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invocation_contract_completion_and_staleness() {
        let temp = tempfile::tempdir().unwrap();
        let session = temp.path().join("sessions/run/attempt-2");
        fs::create_dir_all(&session).unwrap();
        initialize(
            session.parent().unwrap(),
            &DispatchContext {
                profile: "gah".into(),
                work_id: Some("#1431".into()),
                run_id: "run".into(),
                mode: "improve".into(),
            },
        );
        let identity = crate::execution_identity::ExecutionIdentity::legacy_route(
            "codex",
            Some("requested"),
            "codex",
            Some("routed"),
            None::<String>,
        );
        let guard = InvocationGuard::start(
            &session,
            &identity,
            Some("branch".into()),
            Some("#1431"),
            900,
        );
        let record_path = session.join("running-worker.json");
        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        assert_eq!(artifact["owner_pid"], std::process::id());
        let modified = fs::metadata(&record_path).unwrap().modified().unwrap();
        let now = OffsetDateTime::now_utc();
        let rows = observe(temp.path(), now);
        assert_eq!(
            fs::metadata(&record_path).unwrap().modified().unwrap(),
            modified
        );
        assert!(observe(temp.path(), now + time::Duration::days(2)).is_empty());
        assert_eq!(rows.len(), 1);
        let value = serde_json::to_value(&rows[0]).unwrap();
        for key in [
            "work_id",
            "run_id",
            "mode",
            "backend",
            "runner",
            "backend_instance",
            "requested_model",
            "model",
            "actual_model",
            "node_id",
            "branch",
            "started_at",
            "attempt",
            "last_activity_at",
            "state",
        ] {
            assert!(value.get(key).is_some(), "{key}");
        }
        assert!(value.get("owner_pid").is_none());
        assert_eq!(value["model"], "routed");
        assert_eq!(value["requested_model"], "requested");
        assert!(value["actual_model"].is_null());
        assert_eq!(value["attempt"], 2);
        assert_eq!(rows[0].state, "running");
        assert_eq!(
            observe(temp.path(), now + time::Duration::seconds(901))[0].state,
            "stale"
        );
        // A fresh output write renews activity without status reads doing so.
        let mut aged = rows[0].clone();
        aged.started_at = (now - time::Duration::seconds(901))
            .format(&Rfc3339)
            .unwrap();
        fs::write(
            session.join("running-worker.json"),
            serde_json::to_vec(&aged).unwrap(),
        )
        .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(session.join("running-worker.json"))
            .unwrap()
            .set_modified((now - time::Duration::seconds(901)).into())
            .unwrap();
        assert_eq!(observe(temp.path(), now)[0].state, "stale");
        heartbeat(&session);
        assert_eq!(
            observe(temp.path(), OffsetDateTime::now_utc())[0].state,
            "running"
        );
        assert!(observe(temp.path(), now + time::Duration::days(2)).is_empty());
        let mut dead = serde_json::to_value(&aged).unwrap();
        dead["owner_pid"] = serde_json::json!(i32::MAX);
        fs::write(
            session.join("running-worker.json"),
            serde_json::to_vec(&dead).unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let crashed = observe(temp.path(), now);
            assert_eq!(crashed.len(), 1);
            assert_eq!(crashed[0].run_id, "run");
            assert_eq!(crashed[0].state, "stale");
            assert_eq!(
                observe(temp.path(), now + time::Duration::seconds(901))[0].state,
                "stale"
            );
            assert!(observe(temp.path(), now + time::Duration::days(2)).is_empty());
        }
        fs::write(
            session.join("running-worker.json"),
            serde_json::to_vec(&aged).unwrap(),
        )
        .unwrap();
        fs::write(session.join("backend-output.log"), "Agent output").unwrap();
        assert_eq!(
            observe(temp.path(), OffsetDateTime::now_utc())[0].state,
            "running"
        );

        drop(guard);
        assert!(observe(temp.path(), now).is_empty());
    }
}
