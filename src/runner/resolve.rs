use crate::config::Profile;
use anyhow::Result;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableResolution {
    Found(PathBuf),
    MissingExplicitPath(PathBuf),
    MissingFromPath(String),
    UnknownBackend(String),
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn backend_available(name: &str) -> bool {
    backend_command_name(name)
        .and_then(resolve_executable_on_path)
        .is_some()
}

pub fn backend_available_for_profile(profile: &Profile, name: &str) -> bool {
    matches!(
        resolve_backend_executable(profile, name),
        ExecutableResolution::Found(_)
    )
}

pub fn require_backend_executable(profile: &Profile, backend: &str) -> Result<PathBuf> {
    match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => Ok(path),
        ExecutableResolution::MissingExplicitPath(path) => {
            anyhow::bail!("configured executable '{}' does not exist", path.display())
        }
        ExecutableResolution::MissingFromPath(cmd) => {
            anyhow::bail!("required binary '{}' not found on PATH", cmd)
        }
        ExecutableResolution::UnknownBackend(backend) => {
            anyhow::bail!("unknown backend '{}'", backend)
        }
    }
}

pub fn resolve_backend_executable(profile: &Profile, backend: &str) -> ExecutableResolution {
    let Some(command) = backend_command_name(backend) else {
        return ExecutableResolution::UnknownBackend(backend.to_string());
    };
    if let Some(explicit) = profile.configured_backend_path(backend) {
        let path = PathBuf::from(explicit);
        return if is_executable_path(&path) {
            ExecutableResolution::Found(path)
        } else {
            ExecutableResolution::MissingExplicitPath(path)
        };
    }
    match resolve_executable_on_path(command) {
        Some(path) => ExecutableResolution::Found(path),
        None => ExecutableResolution::MissingFromPath(command.to_string()),
    }
}

pub fn codex_model_args(model: Option<&str>) -> Vec<String> {
    model
        .map(|model| vec!["-m".to_string(), model.to_string()])
        .unwrap_or_default()
}

pub fn filtered_codex_args(extra_args: &[String]) -> Vec<String> {
    filtered_backend_args("codex", extra_args)
}

pub fn extract_model_from_backend_args(backend: &str, args: &[String]) -> Option<String> {
    match backend {
        "codex" => {
            let mut i = 0;
            while i < args.len() {
                let arg = &args[i];
                if matches!(arg.as_str(), "-m" | "--model") {
                    if i + 1 < args.len() {
                        return Some(args[i + 1].clone());
                    }
                    break;
                }
                if let Some(val) = arg.strip_prefix("-m=") {
                    return Some(val.to_string());
                }
                if let Some(val) = arg.strip_prefix("--model=") {
                    return Some(val.to_string());
                }
                i += 1;
            }
            None
        }
        "opencode" | "claude" => {
            let mut i = 0;
            while i < args.len() {
                let arg = &args[i];
                if arg == "--model" {
                    if i + 1 < args.len() {
                        return Some(args[i + 1].clone());
                    }
                    break;
                }
                if let Some(val) = arg.strip_prefix("--model=") {
                    return Some(val.to_string());
                }
                i += 1;
            }
            None
        }
        _ => None,
    }
}

pub fn filtered_backend_args(backend: &str, extra_args: &[String]) -> Vec<String> {
    let mut filtered = Vec::with_capacity(extra_args.len());
    let mut i = 0;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        match backend {
            "codex" => {
                if matches!(arg.as_str(), "-m" | "--model") {
                    i += 2;
                    continue;
                }
                if arg.starts_with("-m=") || arg.starts_with("--model=") {
                    i += 1;
                    continue;
                }
            }
            "opencode" | "claude" => {
                if arg == "--model" {
                    i += 2;
                    continue;
                }
                if arg.starts_with("--model=") {
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }
        filtered.push(arg.clone());
        i += 1;
    }
    filtered
}

pub fn extract_model_from_args(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if matches!(arg.as_str(), "-m" | "--model") {
            if i + 1 < args.len() {
                return Some(args[i + 1].clone());
            }
            break;
        }
        if let Some(val) = arg.strip_prefix("-m=") {
            return Some(val.to_string());
        }
        if let Some(val) = arg.strip_prefix("--model=") {
            return Some(val.to_string());
        }
        i += 1;
    }
    None
}

fn backend_command_name(name: &str) -> Option<&'static str> {
    match name {
        "openhands" | "cloud-coder" | "auto" => Some("openhands"),
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "agy" => Some("agy"),
        "agy-main" => Some("agy-main"),
        "agy-second" => Some("agy-second"),
        "vibe" => Some("vibe"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

fn resolve_executable_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_path(candidate))
}

pub fn is_executable_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use crate::test_support::PathGuard;
    use std::path::PathBuf;

    #[test]
    fn resolve_backend_executable_prefers_explicit_path() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.claude_path = Some(f.bin_dir.join("claude-explicit").display().to_string());
        let _guard = PathGuard::set("");

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude-explicit"))
        );
    }

    #[test]
    fn resolve_backend_executable_falls_back_to_path_when_unset() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude", "#!/bin/sh\nexit 0\n");
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_invalid_explicit_path_is_unavailable() {
        let mut profile = test_profile();
        profile.claude_path = Some("/definitely/missing/claude".into());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::MissingExplicitPath(PathBuf::from("/definitely/missing/claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_supports_codex_and_agy_paths() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "codex-explicit", "#!/bin/sh\nexit 0\n");
        make_fake_bin(&f.bin_dir, "agy-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.codex_path = Some(f.bin_dir.join("codex-explicit").display().to_string());
        profile.agy_path = Some(f.bin_dir.join("agy-explicit").display().to_string());

        assert_eq!(
            resolve_backend_executable(&profile, "codex"),
            ExecutableResolution::Found(f.bin_dir.join("codex-explicit"))
        );
        assert_eq!(
            resolve_backend_executable(&profile, "agy"),
            ExecutableResolution::Found(f.bin_dir.join("agy-explicit"))
        );
    }

    #[test]
    fn test_extract_model_from_args() {
        assert_eq!(
            extract_model_from_args(&[
                "--some-flag".to_string(),
                "-m".to_string(),
                "gpt-5.4-mini".to_string()
            ]),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_args(&["--model=gpt-5.4".to_string(), "-c".to_string()]),
            Some("gpt-5.4".to_string())
        );
        assert_eq!(
            extract_model_from_args(&["-m=gpt-5.4-mini".to_string()]),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(extract_model_from_args(&["--some-flag".to_string()]), None);
        assert_eq!(extract_model_from_args(&["-m".to_string()]), None);
    }

    #[test]
    fn test_extract_model_from_backend_args() {
        assert_eq!(
            extract_model_from_backend_args(
                "codex",
                &[
                    "--some-flag".to_string(),
                    "-m".to_string(),
                    "gpt-5.4-mini".to_string()
                ]
            ),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("codex", &["--model=gpt-5.4".to_string()]),
            Some("gpt-5.4".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("codex", &["-m=gpt-5.4-mini".to_string()]),
            Some("gpt-5.4-mini".to_string())
        );

        assert_eq!(
            extract_model_from_backend_args(
                "opencode",
                &[
                    "--some-flag".to_string(),
                    "-m".to_string(),
                    "gpt-5.4-mini".to_string()
                ]
            ),
            None
        );
        assert_eq!(
            extract_model_from_backend_args(
                "opencode",
                &["--model".to_string(), "gpt-5.4-mini".to_string()]
            ),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("claude", &["--model=gpt-5.4".to_string()]),
            Some("gpt-5.4".to_string())
        );
    }

    #[test]
    fn test_filtered_backend_args() {
        let args = vec![
            "-m".to_string(),
            "stale-model".to_string(),
            "--model=another-stale".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        let filtered = filtered_backend_args("codex", &args);
        assert_eq!(filtered, vec!["--format".to_string(), "json".to_string()]);

        let args = vec![
            "-m".to_string(),
            "stale-model".to_string(),
            "--model=another-stale".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        let filtered = filtered_backend_args("opencode", &args);
        assert_eq!(
            filtered,
            vec![
                "-m".to_string(),
                "stale-model".to_string(),
                "--format".to_string(),
                "json".to_string()
            ]
        );
    }

    #[test]
    fn backend_available_false_for_unknown_backend_name() {
        assert!(!backend_available("not-a-real-backend"));
    }
}
