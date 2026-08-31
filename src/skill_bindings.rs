//! Central skill-bank binding resolution for backend dispatch (#965).
//!
//! The server owns the bank and project overrides. A central node reads the
//! same JSON file directly; a worker asks the central API and keeps one
//! last-known-good response per profile/backend instance.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub id: String,
    pub version: String,
    pub content: String,
    #[serde(default)]
    pub backends: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillResolution {
    pub profile: String,
    pub backend: String,
    pub instance: Option<String>,
    pub source: String,
    pub skills: Vec<ResolvedSkill>,
}

#[derive(Deserialize)]
struct SkillBank {
    #[serde(default)]
    skills: Vec<ResolvedSkill>,
    #[serde(default)]
    bindings: HashMap<String, Vec<String>>,
    #[serde(rename = "bindingOverrides")]
    binding_overrides: Option<Vec<String>>,
}

fn target_label(backend: &str, instance: Option<&str>) -> String {
    instance
        .map(|instance| format!("instance:{instance}"))
        .unwrap_or_else(|| format!("backend:{backend}"))
}

fn profile_label(profile: &str, target: &str) -> String {
    format!("profile:{profile}:{target}")
}

fn version_cmp(left: &str, right: &str) -> Ordering {
    fn parse(version: &str) -> Option<(Vec<u64>, Option<Vec<String>>)> {
        let without_build = version.split_once('+').map_or(version, |(core, _)| core);
        let (core, prerelease) =
            without_build
                .split_once('-')
                .map_or((without_build, None), |(core, prerelease)| {
                    (
                        core,
                        Some(prerelease.split('.').map(str::to_string).collect()),
                    )
                });
        let numbers = core
            .split('.')
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        Some((numbers, prerelease))
    }
    match (parse(left), parse(right)) {
        (Some((mut left_core, left_pre)), Some((mut right_core, right_pre))) => {
            let width = left_core.len().max(right_core.len());
            left_core.resize(width, 0);
            right_core.resize(width, 0);
            let core = left_core.cmp(&right_core);
            if !core.is_eq() {
                return core;
            }
            match (left_pre, right_pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => {
                    for (left, right) in left.iter().zip(&right) {
                        let part = match (left.parse::<u64>(), right.parse::<u64>()) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => left.cmp(right),
                        };
                        if !part.is_eq() {
                            return part;
                        }
                    }
                    left.len().cmp(&right.len())
                }
            }
        }
        _ => left.cmp(right),
    }
}

fn resolve_bank(
    bank: SkillBank,
    profile: &str,
    backend: &str,
    instance: Option<&str>,
) -> Result<SkillResolution> {
    let binding_overrides = bank.binding_overrides.unwrap_or_else(|| {
        bank.bindings
            .values()
            .flatten()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    });
    let profile_instance =
        instance.map(|value| profile_label(profile, &target_label(backend, Some(value))));
    let profile_backend = profile_label(profile, &target_label(backend, None));
    let canonical_instance = instance.map(|value| target_label(backend, Some(value)));
    let canonical_backend = target_label(backend, None);
    let label = profile_instance
        .iter()
        .chain(std::iter::once(&profile_backend))
        .chain(canonical_instance.iter())
        .chain(std::iter::once(&canonical_backend))
        .find(|label| binding_overrides.contains(label))
        .cloned()
        .unwrap_or(canonical_backend);

    let mut newest = HashMap::<String, ResolvedSkill>::new();
    for skill in bank.skills {
        let replace = newest
            .get(&skill.id)
            .is_none_or(|current| version_cmp(&skill.version, &current.version).is_gt());
        if replace {
            newest.insert(skill.id.clone(), skill);
        }
    }
    let mut skills = bank
        .bindings
        .iter()
        .filter(|(_, labels)| labels.contains(&label))
        .map(|(id, _)| {
            newest
                .remove(id)
                .with_context(|| format!("bound skill '{id}' does not exist in the central bank"))
        })
        .collect::<Result<Vec<_>>>()?;
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    for skill in &skills {
        if !skill.backends.is_empty() && !skill.backends.iter().any(|value| value == backend) {
            anyhow::bail!("skill '{}' does not support backend '{backend}'", skill.id);
        }
    }
    Ok(SkillResolution {
        profile: profile.to_string(),
        backend: backend.to_string(),
        instance: instance.map(str::to_string),
        source: if label.starts_with("profile:") {
            "profile"
        } else {
            "canonical"
        }
        .to_string(),
        skills,
    })
}

fn local_bank_path() -> PathBuf {
    std::env::var("GAH_SKILL_BANK_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::config::default_config_dir().join("skills.json"))
}

fn cache_path(profile: &str, backend: &str, instance: Option<&str>) -> PathBuf {
    let root = std::env::var("GAH_SKILL_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::config::default_config_dir().join("skill-cache"));
    let digest = Sha256::digest(format!("{profile}\0{backend}\0{}", instance.unwrap_or("")));
    root.join(format!("{digest:x}.json"))
}

fn write_cache(path: &Path, resolution: &SkillResolution) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating skill cache {}", parent.display()))?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(resolution)?)
        .with_context(|| format!("writing skill cache {}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("installing skill cache {}", path.display()))
}

fn read_cache(path: &Path) -> Result<SkillResolution> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading last-known-good skill cache {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing last-known-good skill cache {}", path.display()))
}

fn validate_resolution(
    resolution: &SkillResolution,
    profile: &str,
    backend: &str,
    instance: Option<&str>,
) -> Result<()> {
    if resolution.profile != profile
        || resolution.backend != backend
        || resolution.instance.as_deref() != instance
    {
        anyhow::bail!("central skill resolution does not match the requested target");
    }
    let ids = resolution
        .skills
        .iter()
        .map(|skill| skill.id.as_str())
        .collect::<HashSet<_>>();
    if ids.len() != resolution.skills.len() {
        anyhow::bail!("central skill resolution contains duplicate skill ids");
    }
    if let Some(skill) = resolution.skills.iter().find(|skill| {
        !skill.backends.is_empty() && !skill.backends.iter().any(|value| value == backend)
    }) {
        anyhow::bail!("skill '{}' does not support backend '{backend}'", skill.id);
    }
    Ok(())
}

trait SkillTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>>;
}

struct CurlSkillTransport;

impl SkillTransport for CurlSkillTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>> {
        let response = crate::curl_http::request("GET", url, None, token, 15)?;
        if !(200..300).contains(&response.status) {
            anyhow::bail!(
                "central skill API returned HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            );
        }
        Ok(response.body)
    }
}

fn resolve_remote(
    transport: &dyn SkillTransport,
    central_url: &str,
    profile: &str,
    backend: &str,
    instance: Option<&str>,
    cache: &Path,
) -> Result<SkillResolution> {
    let fetched: Result<SkillResolution> = (|| {
        let mut url = Url::parse(central_url).context("parsing registry_central_url")?;
        url.set_path(&format!(
            "{}/api/skills/resolve",
            url.path().trim_end_matches('/')
        ));
        url.query_pairs_mut()
            .append_pair("profile", profile)
            .append_pair("backend", backend);
        if let Some(instance) = instance {
            url.query_pairs_mut().append_pair("instance", instance);
        }
        let token = std::env::var("COORDINATOR_TOKEN").ok();
        let body = transport.get(url.as_str(), token.as_deref())?;
        let resolution: SkillResolution =
            serde_json::from_slice(&body).context("parsing central skill resolution")?;
        validate_resolution(&resolution, profile, backend, instance)?;
        write_cache(cache, &resolution)?;
        Ok(resolution)
    })();
    match fetched {
        Ok(resolution) => Ok(resolution),
        Err(error) => match read_cache(cache) {
            Ok(cached) => {
                validate_resolution(&cached, profile, backend, instance)?;
                eprintln!(
                    "gah: central skill resolution failed ({error:#}); using last-known-good cache {}",
                    cache.display()
                );
                Ok(cached)
            }
            Err(cache_error) => Err(error.context(format!(
                "no usable last-known-good skill cache: {cache_error:#}"
            ))),
        },
    }
}

pub fn resolve(
    defaults: &crate::config::Defaults,
    profile: &str,
    backend: &str,
    instance: Option<&str>,
) -> Result<SkillResolution> {
    if let Some(central_url) = defaults.registry_central_url.as_deref() {
        return resolve_remote(
            &CurlSkillTransport,
            central_url,
            profile,
            backend,
            instance,
            &cache_path(profile, backend, instance),
        );
    }
    let path = local_bank_path();
    if !path.exists() {
        return Ok(SkillResolution {
            profile: profile.to_string(),
            backend: backend.to_string(),
            instance: instance.map(str::to_string),
            source: "canonical".to_string(),
            skills: vec![],
        });
    }
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading central skill bank {}", path.display()))?;
    let bank = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing central skill bank {}", path.display()))?;
    resolve_bank(bank, profile, backend, instance)
}

pub fn materialize_args(
    backend: &str,
    configured_args: &[String],
    skills: &[ResolvedSkill],
) -> Vec<String> {
    let mut args =
        crate::runner::resolve::filtered_configured_backend_args(backend, configured_args);
    if matches!(backend, "hermes" | "openhands") && !skills.is_empty() {
        args.push("--skills".into());
        args.extend(skills.iter().map(|skill| skill.id.clone()));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, version: &str) -> ResolvedSkill {
        ResolvedSkill {
            id: id.into(),
            version: version.into(),
            content: format!("{id} instructions"),
            backends: vec!["hermes".into()],
        }
    }

    #[test]
    fn profile_override_replaces_canonical_and_resolves_newest_version() {
        let bank = SkillBank {
            skills: vec![
                skill("alpha", "1.0.0"),
                skill("alpha", "2.0.0"),
                skill("beta", "1.0.0"),
            ],
            bindings: HashMap::from([
                ("alpha".into(), vec!["backend:hermes".into()]),
                ("beta".into(), vec!["profile:repo:backend:hermes".into()]),
            ]),
            binding_overrides: Some(vec![
                "backend:hermes".into(),
                "profile:repo:backend:hermes".into(),
            ]),
        };

        let resolved = resolve_bank(bank, "repo", "hermes", None).unwrap();

        assert_eq!(resolved.source, "profile");
        assert_eq!(
            resolved
                .skills
                .iter()
                .map(|skill| skill.id.as_str())
                .collect::<Vec<_>>(),
            ["beta"]
        );
    }

    #[test]
    fn missing_bound_skill_fails_before_dispatch() {
        let bank = SkillBank {
            skills: vec![],
            bindings: HashMap::from([("missing".into(), vec!["backend:hermes".into()])]),
            binding_overrides: Some(vec!["backend:hermes".into()]),
        };

        assert!(resolve_bank(bank, "repo", "hermes", None)
            .unwrap_err()
            .to_string()
            .contains("does not exist"));
    }

    #[test]
    fn native_skill_args_replace_deprecated_profile_flags() {
        let args = materialize_args(
            "hermes",
            &["--skills=legacy".into(), "--trace".into()],
            &[skill("alpha", "2.0.0")],
        );
        assert_eq!(args, ["--trace", "--skills", "alpha"]);
        assert_eq!(
            materialize_args("codex", &["--trace".into()], &[skill("alpha", "2.0.0")]),
            ["--trace"]
        );
    }

    struct Offline;

    impl SkillTransport for Offline {
        fn get(&self, _url: &str, _token: Option<&str>) -> Result<Vec<u8>> {
            anyhow::bail!("offline")
        }
    }

    #[test]
    fn remote_outage_uses_last_known_good_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("skills.json");
        let expected = SkillResolution {
            profile: "repo".into(),
            backend: "hermes".into(),
            instance: None,
            source: "canonical".into(),
            skills: vec![skill("alpha", "1.0.0")],
        };
        write_cache(&cache, &expected).unwrap();

        let actual = resolve_remote(
            &Offline,
            "http://central:3773",
            "repo",
            "hermes",
            None,
            &cache,
        )
        .unwrap();

        assert_eq!(actual, expected);
    }
}
