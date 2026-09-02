// Command execution for `gah skills` (issue #966, #863 gap 2).
//
// Mirrors `gah quota`'s split: an explicit `refresh` performs the actual
// bounded backend query and persists the result; `status` is a read-only
// projection of the durable store plus the current bound resolution, safe
// to call as often as `gah status` itself (no subprocess spawn, no network
// call -- see `skill_bindings::resolve_cached_only`).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::backend_kind::BackendKind;
use crate::cli::args::SkillsCommands;
use crate::{config, skill_bindings, skill_inventory};

struct ResolvedInstance {
    kind: BackendKind,
    executable: PathBuf,
    /// Raw, possibly-aliased backend string (e.g. "agy-second") as declared
    /// on the instance -- the same string `skill_bindings::resolve` and
    /// every other read path key off, kept distinct from `kind` (which is
    /// the canonical `BackendKind` needed only to pick a `BackendRunner`).
    logical_backend: String,
    state_root: Option<PathBuf>,
}

fn resolve_instance(
    profile: &config::Profile,
    defaults: &config::Defaults,
    instance_name: &str,
) -> Result<ResolvedInstance> {
    let routing = profile.effective_routing(defaults);
    let instance = routing
        .backend_instances
        .get(instance_name)
        .with_context(|| {
            format!("no backend instance named '{instance_name}' is configured for this profile")
        })?;
    let logical_backend = instance
        .logical_backend
        .clone()
        .unwrap_or_else(|| instance.runner_kind.clone());
    let kind = BackendKind::parse(&instance.runner_kind)
        .map_err(|e| anyhow::anyhow!("backend instance '{instance_name}': {e}"))?;
    let executable = match instance.executable.as_deref() {
        Some(path) if crate::runner::is_executable_path(Path::new(path)) => PathBuf::from(path),
        Some(path) => bail!(
            "configured executable '{path}' for instance '{instance_name}' does not exist or is not executable"
        ),
        None => match crate::runner::resolve_backend_executable(profile, &instance.runner_kind) {
            crate::runner::ExecutableResolution::Found(path) => path,
            other => bail!(
                "could not resolve an executable for backend '{logical_backend}' (instance '{instance_name}'): {other:?}"
            ),
        },
    };
    Ok(ResolvedInstance {
        kind,
        executable,
        logical_backend,
        state_root: instance.state_root.as_deref().map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendInstanceConfig;

    #[test]
    fn resolve_instance_uses_declared_runner_kind_for_arbitrary_logical_alias() {
        let mut profile = crate::runner::backends::test_util::test_profile();
        profile.routing.backend_instances.insert(
            "research".into(),
            BackendInstanceConfig {
                runner_kind: "hermes".into(),
                logical_backend: Some("hermes-research".into()),
                executable: Some(
                    std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..Default::default()
            },
        );

        let resolved =
            resolve_instance(&profile, &crate::config::Defaults::default(), "research").unwrap();

        assert_eq!(resolved.kind, BackendKind::Hermes);
        assert_eq!(resolved.logical_backend, "hermes-research");
    }
}

pub fn run(command: SkillsCommands) -> Result<()> {
    match command {
        SkillsCommands::Refresh {
            profile: profile_name,
            instance: instance_name,
            store_path: store_arg,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let profile = config::get_profile(&cfg, &profile_name)?;
            let resolved = resolve_instance(profile, &cfg.defaults, &instance_name)?;
            let path = store_arg
                .map(PathBuf::from)
                .unwrap_or_else(skill_inventory::store_path);
            let record = skill_inventory::refresh_and_store(
                resolved.kind,
                &profile_name,
                &resolved.logical_backend,
                Some(instance_name.as_str()),
                &resolved.executable,
                resolved.state_root.as_deref(),
                &path,
                time::OffsetDateTime::now_utc(),
            )?;
            match &record.observed_skill_ids {
                Some(ids) => println!(
                    "Observed {} skill(s) on instance '{instance_name}' ({}).",
                    ids.len(),
                    resolved.logical_backend
                ),
                None => println!(
                    "Instance '{instance_name}' ({}) did not self-report (unknown; nothing fabricated).",
                    resolved.logical_backend
                ),
            }
        }
        SkillsCommands::Status {
            profile: profile_name,
            instance: instance_filter,
            json,
            store_path: store_arg,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let profile = config::get_profile(&cfg, &profile_name)?;
            let routing = profile.effective_routing(&cfg.defaults);
            let path = store_arg
                .map(PathBuf::from)
                .unwrap_or_else(skill_inventory::store_path);
            // Propagated, not defaulted to empty: a store read failure here
            // must surface to the operator running this command directly,
            // not silently masquerade as "no observations exist yet".
            let records = skill_inventory::load(&path)
                .with_context(|| format!("loading skill inventory store {}", path.display()))?;
            let now = time::OffsetDateTime::now_utc();

            let mut names: Vec<&String> = routing.backend_instances.keys().collect();
            names.sort();
            let mut views = Vec::new();
            for name in names {
                if instance_filter.as_deref().is_some_and(|f| f != name) {
                    continue;
                }
                let instance = &routing.backend_instances[name];
                let logical_backend = instance
                    .logical_backend
                    .clone()
                    .unwrap_or_else(|| instance.runner_kind.clone());
                // Non-blocking (#966 AC6): reads the local bank file or the
                // last-known-good cache, never a live HTTP call against a
                // central registry.
                let bound = skill_bindings::resolve_cached_only(
                    &cfg.defaults,
                    &profile_name,
                    &logical_backend,
                    Some(name.as_str()),
                )
                .map(|r| r.skills.into_iter().map(|s| s.id).collect())
                .ok();
                let record = skill_inventory::latest_for(
                    &records,
                    &profile_name,
                    &logical_backend,
                    Some(name.as_str()),
                );
                views.push(skill_inventory::view(
                    bound,
                    record,
                    &logical_backend,
                    Some(name.as_str()),
                    now,
                ));
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
            } else if views.is_empty() {
                println!("No matching backend instances configured for profile '{profile_name}'.");
            } else {
                for view in &views {
                    let bound = match &view.bound_skill_ids {
                        Some(ids) => format!("{ids:?}"),
                        None => "unknown".to_string(),
                    };
                    let observed = match &view.observed_skill_ids {
                        Some(ids) => format!("{ids:?}"),
                        None => "unknown".to_string(),
                    };
                    let stale = match view.observation_stale {
                        Some(true) => " (STALE)",
                        Some(false) => "",
                        None => "",
                    };
                    println!(
                        "{} {}: bound={bound} observed={observed}{stale}",
                        view.backend,
                        view.backend_instance.as_deref().unwrap_or("-"),
                    );
                    if let Some(drift) = &view.drift {
                        if !drift.is_empty() {
                            println!(
                                "  drift: bound_not_observed={:?} observed_not_bound={:?}",
                                drift.bound_not_observed, drift.observed_not_bound
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
