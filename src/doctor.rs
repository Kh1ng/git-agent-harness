use crate::config::{self, Defaults, GahConfig, Profile};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn run(profile_name: Option<&str>, config_path: Option<&str>) -> Result<()> {
    let resolved = config::resolve_config_path(config_path);
    let cfg = config::load(config_path)?;
    let profiles = selected_profiles(&cfg, profile_name)?;

    println!("Config: {}", resolved.display());
    print_check(CheckStatus::Pass, "config", "loaded successfully");

    let mut failed = false;
    for (name, profile) in profiles {
        println!("\n[{}]", name);
        failed |= !check_profile(&cfg.defaults, profile);
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
            claude_args: vec![],
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            routing: RoutingPolicy::default(),
        }
    }

    #[test]
    fn doctor_push_url_check_accepts_self_hosted_gitlab() {
        assert!(check_push_url(&gitlab_profile(Some(
            "https://gitlab.example.internal/api/v4"
        ))));
    }
}
