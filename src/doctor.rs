use crate::config::{self, Defaults, GahConfig, Profile};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn run_with_validate(
    profile_name: Option<&str>,
    config_path: Option<&str>,
    validate: bool,
) -> Result<()> {
    let resolved = config::resolve_config_path(config_path);
    let cfg = config::load(config_path)?;
    let profiles = selected_profiles(&cfg, profile_name)?;

    println!("Config: {}", resolved.display());
    print_check(CheckStatus::Pass, "config", "loaded successfully");

    let mut failed = false;
    for (name, profile) in profiles {
        println!("\n[{}]", name);
        failed |= !check_profile(&cfg.defaults, profile);
        if validate {
            failed |= !check_validation_commands(profile);
            failed |= !check_env_files(profile);
            failed |= !check_backend_executables(&cfg.defaults, profile);
            failed |= !check_review_capabilities(&cfg, profile);
            failed |= !check_merge_policy(profile);
        }
    }

    if failed {
        anyhow::bail!("doctor found failing checks");
    }
    Ok(())
}

fn selected_profiles<'a>(
    cfg: &'a GahConfig,
    profile_name: Option<&str>,
) -> Result<Vec<(String, &'a Profile)>> {
    if let Some(name) = profile_name {
        return Ok(vec![(name.to_string(), config::get_profile(cfg, name)?)]);
    }
    let mut profiles: Vec<_> = cfg.profiles.iter().map(|(k, v)| (k.clone(), v)).collect();
    profiles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(profiles)
}

fn check_profile(defaults: &Defaults, profile: &Profile) -> bool {
    let mut failed = false;
    failed |= !check_repo(profile);
    failed |= !check_provider_cli(profile);
    failed |= !check_provider_token(profile);
    failed |= !check_push_url(profile);
    failed |= !check_writable_path("artifact_root", Path::new(&profile.artifact_root));
    if !defaults.worktree_base.trim().is_empty() {
        failed |= !check_writable_path("worktree_base", Path::new(&defaults.worktree_base));
    }
    failed |= !check_manager_memory(profile);
    !failed
}

fn check_repo(profile: &Profile) -> bool {
    let repo = Path::new(&profile.local_path);
    if !repo.exists() {
        print_check(
            CheckStatus::Fail,
            "repo path",
            &format!("missing {}", repo.display()),
        );
        return false;
    }
    if !repo.join(".git").exists() {
        print_check(
            CheckStatus::Fail,
            "git repo",
            &format!("{} is not a git repository", repo.display()),
        );
        return false;
    }
    print_check(CheckStatus::Pass, "repo path", &repo.display().to_string());
    true
}

fn check_provider_cli(profile: &Profile) -> bool {
    let Some(bin) = profile.provider_cli() else {
        print_check(
            CheckStatus::Warn,
            "provider CLI",
            "no provider-specific CLI check",
        );
        return true;
    };
    if which(bin) {
        print_check(CheckStatus::Pass, "provider CLI", bin);
        true
    } else {
        print_check(
            CheckStatus::Fail,
            "provider CLI",
            &format!("missing {}", bin),
        );
        false
    }
}

fn check_provider_token(profile: &Profile) -> bool {
    let vars = profile.pat_env_names();
    if vars.is_empty() {
        print_check(
            CheckStatus::Warn,
            "provider token",
            "no known token convention",
        );
        return true;
    }
    if profile.pat().is_empty() {
        print_check(
            CheckStatus::Fail,
            "provider token",
            &format!("set one of {}", vars.join(", ")),
        );
        false
    } else {
        print_check(
            CheckStatus::Pass,
            "provider token",
            &format!("found {}", vars.join(" or ")),
        );
        true
    }
}

fn check_push_url(profile: &Profile) -> bool {
    match profile.push_url() {
        Ok(url) => {
            print_check(CheckStatus::Pass, "push URL", &url);
            true
        }
        Err(err) => {
            print_check(CheckStatus::Fail, "push URL", &format!("{:#}", err));
            false
        }
    }
}

fn check_writable_path(label: &str, path: &Path) -> bool {
    if let Err(err) = fs::create_dir_all(path) {
        print_check(
            CheckStatus::Fail,
            label,
            &format!("cannot create {}: {}", path.display(), err),
        );
        return false;
    }
    let probe = path.join(".gah-write-test");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            print_check(CheckStatus::Pass, label, &path.display().to_string());
            true
        }
        Err(err) => {
            print_check(
                CheckStatus::Fail,
                label,
                &format!("not writable {}: {}", path.display(), err),
            );
            false
        }
    }
}

fn check_manager_memory(profile: &Profile) -> bool {
    let path = Path::new(&profile.local_path).join("docs/MANAGER_MEMORY.md");
    if path.exists() {
        print_check(
            CheckStatus::Pass,
            "manager memory",
            &path.display().to_string(),
        );
        true
    } else {
        print_check(
            CheckStatus::Fail,
            "manager memory",
            &format!("missing {}", path.display()),
        );
        false
    }
}

/// TICKET-076: `--validate` extends the existing checks with execution
/// prerequisites doctor doesn't already cover -- whether the configured
/// validation commands and backend executables actually resolve, and
/// whether declared env files exist. Deliberately does not re-check repo
/// path, provider CLI/token, push URL, or writable roots -- `check_profile`
/// already covers those.
fn check_validation_commands(profile: &Profile) -> bool {
    if profile.validation_commands.is_empty() {
        print_check(CheckStatus::Warn, "validation commands", "none configured");
        return true;
    }
    let mut failed = false;
    for cmd in &profile.validation_commands {
        let Some(bin) = cmd.split_whitespace().next() else {
            continue;
        };
        if which(bin) || Path::new(bin).exists() {
            print_check(CheckStatus::Pass, "validation command", cmd);
        } else {
            print_check(
                CheckStatus::Fail,
                "validation command",
                &format!("'{}' not resolvable (from: {})", bin, cmd),
            );
            failed = true;
        }
    }
    !failed
}

fn check_env_files(profile: &Profile) -> bool {
    let mut failed = false;
    for (label, path) in [
        ("env_file", profile.env_file.as_deref()),
        ("env_file_prod", profile.env_file_prod.as_deref()),
    ] {
        let Some(path) = path else { continue };
        if Path::new(path).exists() {
            print_check(CheckStatus::Pass, label, path);
        } else {
            print_check(CheckStatus::Fail, label, &format!("missing {}", path));
            failed = true;
        }
    }
    !failed
}

fn check_backend_executables(defaults: &Defaults, profile: &Profile) -> bool {
    let backends = configured_backends(defaults, profile);
    if backends.is_empty() {
        print_check(
            CheckStatus::Warn,
            "backend executables",
            "no backend configured in routing policy",
        );
        return true;
    }
    let mut failed = false;
    for backend in backends {
        match crate::runner::resolve_backend_executable(profile, &backend) {
            crate::runner::ExecutableResolution::Found(path) => {
                print_check(
                    CheckStatus::Pass,
                    "backend executable",
                    &format!("{}: {}", backend, path.display()),
                );
            }
            other => {
                print_check(
                    CheckStatus::Fail,
                    "backend executable",
                    &format!("{}: {:?}", backend, other),
                );
                failed = true;
            }
        }
    }
    !failed
}

/// Issue #124 / TICKET-127: validates that the resolved merge policy is
/// internally consistent with the provider. `gitlab_mwps` only makes sense
/// for a GitLab-backed profile; flag it on any other provider so the operator
/// discovers the misconfiguration in `doctor` rather than at merge time.
pub(crate) fn check_merge_policy(profile: &Profile) -> bool {
    let policy = match &profile.routing.merge_policy {
        None => {
            // No profile-level override: the default (`auto`) always applies.
            print_check(CheckStatus::Pass, "merge policy", "default (auto)");
            return true;
        }
        Some(p) => p,
    };
    let label = policy.as_str();
    if *policy == config::MergePolicy::GitlabMwps && profile.provider != "gitlab" {
        print_check(
            CheckStatus::Fail,
            "merge policy",
            &format!(
                "merge_policy '{}' requires provider 'gitlab' but profile uses '{}'",
                label, profile.provider
            ),
        );
        return false;
    }
    print_check(CheckStatus::Pass, "merge policy", label);
    true
}

/// TICKET-105: reuses `dispatch::review_preflight` -- the exact same check
/// the real review invocation runs -- so preflight and actual invocation
/// can never drift into inconsistent configuration.
fn check_review_capabilities(cfg: &GahConfig, profile: &Profile) -> bool {
    let mut backends = std::collections::BTreeSet::new();
    for routing in [&profile.routing, &cfg.defaults.routing] {
        backends.extend(routing.review_required_capabilities.keys().cloned());
    }
    if backends.is_empty() {
        print_check(
            CheckStatus::Warn,
            "review capabilities",
            "no review_required_capabilities configured",
        );
        return true;
    }
    let mut failed = false;
    for backend in backends {
        match crate::dispatch::review_preflight(cfg, profile, &backend) {
            Ok(capabilities) => {
                print_check(
                    CheckStatus::Pass,
                    "review capabilities",
                    &format!("{}: {}", backend, capabilities.join(", ")),
                );
            }
            Err(err) => {
                print_check(
                    CheckStatus::Fail,
                    "review capabilities",
                    &format!("{}: {:#}", backend, err),
                );
                failed = true;
            }
        }
    }
    !failed
}

fn configured_backends(defaults: &Defaults, profile: &Profile) -> Vec<String> {
    let mut backends = std::collections::BTreeSet::new();
    for routing in [&profile.routing, &defaults.routing] {
        for b in [
            &routing.default_backend,
            &routing.pm_backend,
            &routing.improve_backend,
            &routing.review_backend,
            &routing.strong_review_backend,
            &routing.weak_review_backend,
        ]
        .into_iter()
        .flatten()
        {
            backends.insert(b.clone());
        }
        for list in [
            &routing.pm_candidates,
            &routing.improve_candidates,
            &routing.review_candidates,
        ]
        .into_iter()
        .flatten()
        {
            for c in list {
                backends.insert(c.backend.clone());
            }
        }
    }
    backends.into_iter().collect()
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn print_check(status: CheckStatus, label: &str, detail: &str) {
    println!("[{}] {:<16} {}", status.as_str(), label, detail);
}

#[derive(Clone, Copy)]
enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check_push_url;
    use crate::config::{Profile, RoutingPolicy};

    fn gitlab_profile(api_base: Option<&str>) -> Profile {
        Profile {
            prune_older_than_days: None,
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "gitlab".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: api_base.map(str::to_string),
            provider_project_id: Some("42".into()),
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
            openhands_idle_timeout_seconds: None,
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

    #[test]
    fn doctor_push_url_check_accepts_self_hosted_gitlab() {
        assert!(check_push_url(&gitlab_profile(Some(
            "https://gitlab.example.internal/api/v4"
        ))));
    }

    // Issue #124 / TICKET-127: `gitlab_mwps` is only valid on GitLab providers.
    // On a GitHub profile it must be reported as a hard doctor failure.
    #[test]
    fn doctor_rejects_gitlab_mwps_on_non_gitlab_provider() {
        let mut profile = github_profile();
        profile.routing.merge_policy = Some(crate::config::MergePolicy::GitlabMwps);
        assert!(!crate::doctor::check_merge_policy(&profile));

        let mut gitlab = gitlab_profile(None);
        gitlab.routing.merge_policy = Some(crate::config::MergePolicy::GitlabMwps);
        assert!(crate::doctor::check_merge_policy(&gitlab));

        // Non-MWPS policies are valid on every provider.
        let mut github = github_profile();
        github.routing.merge_policy = Some(crate::config::MergePolicy::StopForHuman);
        assert!(crate::doctor::check_merge_policy(&github));
    }

    fn github_profile() -> Profile {
        let mut p = gitlab_profile(None);
        p.provider = "github".into();
        p
    }
}
