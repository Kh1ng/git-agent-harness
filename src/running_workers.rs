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

pub fn initialize(session: &Path, context: &DispatchContext) {
    if let Ok(bytes) = serde_json::to_vec(context) {
        let _ = fs::write(session.join("dispatch-context.json"), bytes);
    }
}

/// Observation failures must not change dispatch behavior. On normal completion
/// Drop removes the record; abrupt death leaves evidence for a stale row.
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
        let written = serde_json::to_vec(&worker).ok().and_then(|bytes| {
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
                let Some(mut row) = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<RunningWorker>(&bytes).ok())
                else {
                    continue;
                };
                let mut last = OffsetDateTime::parse(&row.started_at, &Rfc3339).unwrap_or(now);
                // Only agent output is activity; status reads never renew it.
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
                row.last_activity_at = last.format(&Rfc3339).unwrap_or_default();
                row.state = if (now - last).whole_seconds() >= row.stale_after_seconds as i64 {
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
        let now = OffsetDateTime::now_utc();
        let rows = observe(temp.path(), now);
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
        assert_eq!(observe(temp.path(), now)[0].state, "stale");
        fs::write(session.join("backend-output.log"), "Agent output").unwrap();
        assert_eq!(
            observe(temp.path(), OffsetDateTime::now_utc())[0].state,
            "running"
        );

        drop(guard);
        assert!(observe(temp.path(), now).is_empty());
    }
}
