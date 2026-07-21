use crate::config::{self, Defaults, GahConfig, Profile};
use crate::provider::provider_command;
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
            failed |= !check_reviewer_config(&cfg.defaults, profile);
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
    failed |= !check_provider_auth(profile);
    failed |= !check_push_url(profile);
    failed |= !check_writable_path("artifact_root", Path::new(&profile.artifact_root));
    if !defaults.worktree_base.trim().is_empty() {
        failed |= !check_writable_path("worktree_base", Path::new(&defaults.worktree_base));
    }
    failed |= !check_manager_memory(profile);
    failed |= !check_candidate_model_consistency(defaults, profile);
    failed |= !check_generated_artifact_policy(profile);
    !failed
}

fn check_generated_artifact_policy(profile: &Profile) -> bool {
    let patterns = &profile.publishing.generated_artifact_deny_patterns;
    if patterns.is_empty() {
        print_check(
            CheckStatus::Warn,
            "generated artifact policy",
            "disabled by explicit empty pattern list",
        );
        return true;
    }
    if let Err(error) = crate::generated_artifacts::validate_patterns(patterns) {
        print_check(
            CheckStatus::Fail,
            "generated artifact policy",
            &format!("{error:#}"),
        );
        return false;
    }
    print_check(
        CheckStatus::Pass,
        "generated artifact policy",
        &format!("{} pattern(s): {}", patterns.len(), patterns.join(", ")),
    );
    true
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

/// Provider-neutral authentication result shape shared by the GitHub and
/// GitLab adapters. `doctor` accepts either an explicit supported token
/// environment variable or a successful, non-secret provider CLI preflight
/// against the exact configured host and project. It fails closed otherwise.
pub(crate) enum ProviderAuthMethod {
    Token,
    ProviderCli,
}

pub(crate) enum ProviderAuthFailure {
    /// No supported token env var and the provider CLI is unavailable.
    NoCredential(String),
    /// Provider CLI cannot authenticate to the exact configured host (wrong
    /// host, expired token, or simply not logged in).
    AuthFailed(String),
    /// Authenticated to the exact host but the configured project is
    /// inaccessible or does not exist.
    ProjectUnavailable(String),
    /// The provider CLI could not reach the host (transport/network error).
    Network(String),
    /// The exact host could not be derived from configuration.
    HostUnconfigured(String),
}

pub(crate) enum ProviderAuthResult {
    Authenticated(ProviderAuthMethod),
    Failed(ProviderAuthFailure),
}

fn check_provider_auth(profile: &Profile) -> bool {
    let vars = profile.pat_env_names();
    let result = match profile.provider.as_str() {
        "gitlab" => gitlab_provider_auth(profile),
        "github" => github_provider_auth(profile),
        _other => {
            // Unknown provider: fall back to the original token-convention check.
            if vars.is_empty() {
                print_check(
                    CheckStatus::Warn,
                    "provider auth",
                    "no known auth convention",
                );
                return true;
            }
            if profile.pat().is_empty() {
                print_check(
                    CheckStatus::Fail,
                    "provider auth",
                    &format!("set one of {}", vars.join(", ")),
                );
                ProviderAuthResult::Failed(ProviderAuthFailure::NoCredential(format!(
                    "set one of {}",
                    vars.join(", ")
                )))
            } else {
                print_check(
                    CheckStatus::Pass,
                    "provider auth",
                    &format!("found {}", vars.join(" or ")),
                );
                ProviderAuthResult::Authenticated(ProviderAuthMethod::Token)
            }
        }
    };

    match result {
        ProviderAuthResult::Authenticated(method) => {
            let detail = match method {
                ProviderAuthMethod::Token => format!("found {}", vars.join(" or ")),
                ProviderAuthMethod::ProviderCli => format!(
                    "provider CLI session for exact {} host/project",
                    profile.provider
                ),
            };
            print_check(CheckStatus::Pass, "provider auth", &detail);
            true
        }
        ProviderAuthResult::Failed(reason) => {
            let detail = match reason {
                ProviderAuthFailure::NoCredential(m) => m,
                ProviderAuthFailure::AuthFailed(m) => m,
                ProviderAuthFailure::ProjectUnavailable(m) => m,
                ProviderAuthFailure::Network(m) => m,
                ProviderAuthFailure::HostUnconfigured(m) => m,
            };
            print_check(CheckStatus::Fail, "provider auth", &detail);
            false
        }
    }
}

/// GitHub adapter: accept a `GITHUB_TOKEN`/`GH_TOKEN` env var, or a successful
/// `gh api` preflight against the exact `github.com` host and project.
fn github_provider_auth(profile: &Profile) -> ProviderAuthResult {
    if !profile.pat().is_empty() {
        return ProviderAuthResult::Authenticated(ProviderAuthMethod::Token);
    }
    let host = "github.com";
    run_provider_cli_preflight("gh", host, &format!("repos/{}", profile.repo), "github")
}

/// GitLab adapter: accept a `GITLAB_PAT`/`GITLAB_PAT2` env var, or a successful
/// `glab api` preflight against the exact configured GitLab host and project.
fn gitlab_provider_auth(profile: &Profile) -> ProviderAuthResult {
    if !profile.pat().is_empty() {
        return ProviderAuthResult::Authenticated(ProviderAuthMethod::Token);
    }
    let Some(host) = gitlab_host(profile) else {
        return ProviderAuthResult::Failed(ProviderAuthFailure::HostUnconfigured(
            "gitlab profile missing provider_api_base; cannot determine the exact host to \
             authenticate against"
                .into(),
        ));
    };
    let project_ref = match profile.provider_project_id.as_deref() {
        Some(id) => id.to_string(),
        None => profile.repo.replace('/', "%2F"),
    };
    run_provider_cli_preflight(
        "glab",
        &host,
        &format!("/projects/{}", project_ref),
        "gitlab",
    )
}

/// The exact GitLab host a `glab` session must be authenticated against,
/// derived from `provider_api_base`. Returns `None` when it cannot be
/// determined (e.g. a GitLab profile without `provider_api_base`).
fn gitlab_host(profile: &Profile) -> Option<String> {
    let base = profile.provider_api_base.as_deref()?.trim();
    let trimmed = base.trim_end_matches('/');
    let without_api = trimmed.strip_suffix("/api/v4").unwrap_or(trimmed);
    let (_, rest) = without_api
        .split_once("://")
        .unwrap_or(("https", without_api));
    let host = rest.split('/').next().unwrap_or("").trim_matches('/');
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Runs a non-secret provider CLI API preflight scoped to the exact host and
/// project, and classifies the result. The CLI reads its own credential store,
/// so no token is ever read, printed, persisted, or copied by `doctor`.
fn run_provider_cli_preflight(
    cli: &str,
    host: &str,
    api_path: &str,
    provider: &str,
) -> ProviderAuthResult {
    if !which(cli) {
        return ProviderAuthResult::Failed(ProviderAuthFailure::NoCredential(format!(
            "{cli} not found on PATH and no {provider} token env var set"
        )));
    }
    let output = match provider_command(cli)
        .args(["api", "--hostname", host, api_path])
        .output()
    {
        Ok(out) => out,
        Err(err) => {
            return ProviderAuthResult::Failed(ProviderAuthFailure::NoCredential(format!(
                "failed to invoke {cli}: {err}"
            )))
        }
    };
    if output.status.success() {
        return ProviderAuthResult::Authenticated(ProviderAuthMethod::ProviderCli);
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ProviderAuthResult::Failed(classify_provider_cli_failure(&combined, cli))
}

/// Maps a failed provider CLI preflight to a classified reason. Network errors
/// are distinguished from auth failures and from project-not-found errors.
fn classify_provider_cli_failure(output: &str, cli: &str) -> ProviderAuthFailure {
    let text = output.to_lowercase();

    // Transport/network failure reaching the provider host.
    if text.contains("error connecting to")
        || text.contains("check your internet connection")
        || text.contains("could not resolve")
        || text.contains("no such host")
        || text.contains("dial tcp")
        || text.contains("connection refused")
        || text.contains("connection reset")
        || text.contains("network is unreachable")
        || text.contains("temporary failure in name resolution")
        || text.contains("lookup ")
        || (text.contains("timeout") && text.contains("deadline"))
    {
        return ProviderAuthFailure::Network(format!(
            "{cli} preflight could not reach the provider host (network error)"
        ));
    }

    // Authenticated to the host, but the configured project is unavailable.
    if text.contains("404")
        || text.contains("not found")
        || text.contains("does not exist")
        || text.contains("repository not found")
        || text.contains("project not found")
        || text.contains("resource not found")
        || text.contains("http 404")
    {
        return ProviderAuthFailure::ProjectUnavailable(format!(
            "{cli} preflight authenticated to the exact host but the configured project is \
             inaccessible or not found"
        ));
    }

    // Auth failure: wrong host, expired token, or not logged in.
    if text.contains("401")
        || text.contains("403")
        || text.contains("unauthorized")
        || text.contains("forbidden")
        || text.contains("token expired")
        || text.contains("must be logged in")
        || text.contains("not logged into")
        || text.contains("missing authentication")
        || text.contains("to authenticate")
        || text.contains("http 401")
        || text.contains("http 403")
    {
        return ProviderAuthFailure::AuthFailed(format!(
            "{cli} preflight could not authenticate to the exact {cli} host (wrong host, \
             expired token, or not logged in)"
        ));
    }

    ProviderAuthFailure::AuthFailed(format!(
        "{cli} preflight failed to authenticate to the exact {cli} host"
    ))
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
/// Issue #123 / TICKET-stabilization: validate the reviewer-tier config.
///
/// Two schemes exist: the new `routine_reviewer` + `escalatory_reviewers`
/// list, and the deprecated single `strong_review_*` / `weak_review_*` pair.
/// They are mutually exclusive -- setting both is a misconfiguration the
/// back-compat shim cannot resolve unambiguously. A routine reviewer (new or
/// legacy) is required for review to have an authority tier.
pub(crate) fn check_reviewer_config(defaults: &Defaults, profile: &Profile) -> bool {
    let routing = profile.effective_routing(defaults);
    let uses_new = routing.routine_reviewer.is_some() || !routing.escalatory_reviewers.is_empty();
    let uses_legacy =
        routing.strong_review_backend.is_some() || routing.weak_review_backend.is_some();

    if uses_new && uses_legacy {
        print_check(
            CheckStatus::Fail,
            "reviewer config",
            "both new (routine_reviewer/escalatory_reviewers) and deprecated \
             (strong_review_*/weak_review_*) reviewer fields are set; they are \
             mutually exclusive -- migrate fully to the new scheme",
        );
        return false;
    }

    match routing.effective_routine_reviewer() {
        Some(r) => print_check(
            CheckStatus::Pass,
            "reviewer config",
            &format!(
                "routine reviewer '{}'{}",
                r.backend,
                r.model
                    .as_deref()
                    .map(|m| format!("/{m}"))
                    .unwrap_or_default()
            ),
        ),
        None => print_check(
            CheckStatus::Warn,
            "reviewer config",
            "no routine reviewer configured (set routine_reviewer or \
             strong_review_backend) -- routine review has no STRONG authority tier",
        ),
    }

    let escalatory = routing.effective_escalatory_reviewers();
    if !escalatory.is_empty() {
        let summary = escalatory
            .iter()
            .map(|c| {
                format!(
                    "{}{}",
                    c.backend,
                    c.model
                        .as_deref()
                        .map(|m| format!("/{m}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        print_check(
            CheckStatus::Pass,
            "reviewer config",
            &format!("escalatory reviewers: {}", summary),
        );
    }
    true
}

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
        if let Some(r) = &routing.routine_reviewer {
            backends.insert(r.backend.clone());
        }
        for list in [
            &routing.pm_candidates,
            &routing.improve_candidates,
            &routing.review_candidates,
            &Some(routing.escalatory_reviewers.clone()),
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

fn check_candidate_model_consistency(defaults: &Defaults, profile: &Profile) -> bool {
    match config::check_profile_candidate_model_consistency(defaults, profile) {
        Ok(()) => {
            print_check(
                CheckStatus::Pass,
                "candidate model",
                "all candidate model labels consistent with profile backend args pins",
            );
            true
        }
        Err(errors) => {
            let mut failed = false;
            for err in &errors {
                print_check(CheckStatus::Fail, "candidate model", err);
                failed = true;
            }
            !failed
        }
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
            delivery_mode: crate::config::DeliveryMode::default(),
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
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
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            max_open_managed_mrs: None,
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
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

    #[test]
    fn doctor_check_candidate_model_consistency() {
        let defaults = crate::config::Defaults::default();

        // 1. Mismatch case: reproducing the gpt-5.6-luna/-m gpt-5.4-mini incident.
        let mut profile = github_profile();
        profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];
        profile.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
            backend: "codex".to_string(),
            model: Some("gpt-5.6-luna".to_string()),
            ..Default::default()
        }]);
        assert!(!super::check_candidate_model_consistency(
            &defaults, &profile
        ));

        // 2. Match case: label and pin agree.
        let mut profile = github_profile();
        profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];
        profile.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
            backend: "codex".to_string(),
            model: Some("gpt-5.4-mini".to_string()),
            ..Default::default()
        }]);
        assert!(super::check_candidate_model_consistency(
            &defaults, &profile
        ));

        // 3. No-pin case.
        let mut profile = github_profile();
        profile.codex_args = vec![];
        profile.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
            backend: "codex".to_string(),
            model: Some("gpt-5.6-luna".to_string()),
            ..Default::default()
        }]);
        assert!(super::check_candidate_model_consistency(
            &defaults, &profile
        ));
    }

    fn github_profile() -> Profile {
        let mut p = gitlab_profile(None);
        p.provider = "github".into();
        p
    }
}
