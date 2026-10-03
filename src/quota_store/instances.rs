//! Native OAuth probes use the explicitly isolated runner account, never a
//! named API key's ambient login or a sibling's default state directory.
use super::{append, maybe_refresh_backend_instance, QuotaObservationRecord};
use crate::{config, execution_identity::ExecutionIdentity};
use anyhow::{bail, Result};
use std::path::Path;
use time::OffsetDateTime;

pub(super) fn refresh(
    profile: &config::Profile,
    now: OffsetDateTime,
    store: &Path,
) -> Vec<std::thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    for (id, instance) in &profile.routing.backend_instances {
        if !instance.enabled()
            || instance.credential_id.is_some()
            || !matches!(instance.runner_kind.as_str(), "codex" | "claude")
        {
            continue;
        }
        let instance = instance.clone();
        let id = id.clone();
        let backend = instance
            .logical_backend
            .clone()
            .unwrap_or_else(|| instance.runner_kind.clone());
        let path = store.to_path_buf();
        let instance_id = id.clone();
        let logical_backend = backend.clone();
        if let Some(handle) =
            maybe_refresh_backend_instance(store, &backend, Some(&id), now, move || {
                let root = instance
                    .state_root
                    .as_deref()
                    .map(Path::new)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "auth_required: named OAuth quota requires an isolated state root"
                        )
                    })?;
                let mut identity = ExecutionIdentity::legacy_candidate(
                    &logical_backend,
                    None::<String>,
                    instance.quota_pool.as_deref(),
                );
                identity.backend_instance = instance_id;
                let records = match instance.runner_kind.as_str() {
                    "claude" => crate::usage::claude::refresh_directory(&root.join(".claude"))?,
                    "codex" => {
                        let crate::runner::ExecutableResolution::Found(executable) =
                            crate::runner::resolve_backend_instance_executable(&instance)
                        else {
                            bail!("native Codex executable unavailable");
                        };
                        let environment = vec![
                            ("HOME".into(), root.to_string_lossy().into_owned()),
                            (
                                "CODEX_HOME".into(),
                                root.join(".codex").to_string_lossy().into_owned(),
                            ),
                        ];
                        let Some(observation) = crate::usage::refresh_codex_quota_with_env(
                            executable.to_string_lossy().as_ref(),
                            None,
                            &environment,
                        )?
                        else {
                            bail!("Codex account usage unavailable");
                        };
                        vec![super::record_from_codex_observation(observation, &identity)]
                    }
                    _ => unreachable!(),
                };
                store_records(&path, records, &identity)
            })
        {
            handles.push(handle);
        }
    }
    handles
}

fn store_records(
    path: &Path,
    mut records: Vec<QuotaObservationRecord>,
    identity: &ExecutionIdentity,
) -> Result<Option<QuotaObservationRecord>> {
    for record in &mut records {
        record.backend = identity.logical_backend.clone();
        record.backend_instance = Some(identity.backend_instance.clone());
        record.quota_pool = identity.quota_pool.clone();
        append(path, record)?;
    }
    Ok(records.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obsolete_named_refresh_does_not_append_a_generic_source_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let handle = super::super::maybe_refresh_source(
            &path,
            "test-obsolete-named-source",
            None,
            Some("removed-source"),
            OffsetDateTime::now_utc(),
            || anyhow::bail!("credential changed during quota check"),
        )
        .unwrap();
        handle.join().unwrap();
        assert!(super::super::load(&path).unwrap().is_empty());
    }
    #[test]
    fn bound_api_instances_skip_oauth_and_missing_isolation_does_not_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let mut profile = config::tests::test_profile_for_notifications();
        profile.routing.backend_instances.clear();
        profile.routing.backend_instances.insert(
            "api-bound".into(),
            config::BackendInstanceConfig {
                runner_kind: "claude".into(),
                credential_id: Some("named-key".into()),
                ..Default::default()
            },
        );
        profile.routing.backend_instances.insert(
            "oauth-missing".into(),
            config::BackendInstanceConfig {
                runner_kind: "claude".into(),
                ..Default::default()
            },
        );
        for handle in refresh(&profile, OffsetDateTime::now_utc(), &path) {
            handle.join().unwrap();
        }
        let records = super::super::load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].backend_instance.as_deref(),
            Some("oauth-missing")
        );
        assert!(records[0]
            .check_error
            .as_deref()
            .unwrap()
            .contains("isolated state root"));
        assert!(records[0].quota_remaining_percent.is_none());
    }
}
