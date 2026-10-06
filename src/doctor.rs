use crate::config::{self, Defaults, GahConfig, Profile};
use crate::provider::provider_command;
use anyhow::Result;
use serde::Serialize;
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn run_with_validate(
    profile_name: Option<&str>,
    config_path: Option<&str>,
    validate: bool,
) -> Result<()> {
    run(profile_name, config_path, validate, false)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DoctorCheck {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub name: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorSnapshot {
    pub schema_version: u32,
    pub generated_at: String,
    pub overall_status: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Default)]
struct CheckCapture {
    profile: Option<String>,
    checks: Vec<DoctorCheck>,
}

thread_local! {
    static CHECK_CAPTURE: RefCell<Option<CheckCapture>> = const { RefCell::new(None) };
}

pub fn run(
    profile_name: Option<&str>,
    config_path: Option<&str>,
    validate: bool,
    json: bool,
) -> Result<()> {
    if json {
        CHECK_CAPTURE.with(|capture| *capture.borrow_mut() = Some(CheckCapture::default()));
    }
    let resolved = config::resolve_config_path(config_path);
    let cfg = match config::load(config_path) {
        Ok(cfg) => cfg,
        Err(error) if json => {
            CHECK_CAPTURE.with(|capture| *capture.borrow_mut() = None);
            let snapshot = DoctorSnapshot {
                schema_version: 1,
                generated_at: generated_at(),
                overall_status: "fail".to_string(),
                checks: vec![DoctorCheck {
                    profile: None,
                    name: "config".to_string(),
                    status: "fail".to_string(),
                    detail: format!("{}: {error:#}", resolved.display()),
                }],
            };
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let profiles = selected_profiles(&cfg, profile_name)?;

    if !json {
        println!("Config: {}", resolved.display());
    }
    print_check(CheckStatus::Pass, "config", "loaded successfully");

    let mut failed = !check_worktree_base(&cfg.defaults);
    for (name, profile) in profiles {
        if json {
            CHECK_CAPTURE.with(|capture| {
                if let Some(capture) = capture.borrow_mut().as_mut() {
                    capture.profile = Some(name.clone());
                }
            });
        } else {
            println!("\n[{}]", name);
        }
        failed |= !check_profile(&cfg.defaults, profile);
        if validate {
            failed |= !check_candidate_models(&cfg.defaults, profile);
            failed |= !check_validation_commands(profile);
            failed |= !check_env_files(profile);
            failed |= !check_backend_executables(&cfg.defaults, profile);
            failed |= !check_review_capabilities(&cfg, profile);
            failed |= !check_merge_policy(profile);
            failed |= !check_reviewer_config(&cfg.defaults, profile);
        }
    }

    if json {
        let checks = CHECK_CAPTURE.with(|capture| {
            capture
                .borrow_mut()
                .take()
                .map(|capture| capture.checks)
                .unwrap_or_default()
        });
        let overall_status = if failed {
            "fail"
        } else if checks.iter().any(|check| check.status == "warn") {
            "warn"
        } else {
            "ok"
        };
        let snapshot = DoctorSnapshot {
            schema_version: 1,
            generated_at: generated_at(),
            overall_status: overall_status.to_string(),
            checks,
        };
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    }

    if failed {
        anyhow::bail!("doctor found failing checks");
    }
    Ok(())
}

fn generated_at() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
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

/// Default-level check that runs once per doctor invocation, outside the
/// per-profile loop, so an unusable worktree base is reported even when the
/// config has no profiles (issue #1366).
///
/// An empty value is tolerated: work GAH itself plans resolves the default
/// at the point of use (`config::effective_worktree_base`), the stored value
/// is never rewritten on the operator's behalf, and chat sessions keep
/// checkout mode for unconfigured profiles. Doctor reports where the work
/// will go instead of failing.
fn check_worktree_base(defaults: &Defaults) -> bool {
    if defaults.worktree_base.trim().is_empty() {
        let resolved = config::effective_worktree_base(defaults);
        return match worktree_base_probe(&resolved) {
            Ok(()) => {
                print_check(
                    CheckStatus::Pass,
                    "worktree_base",
                    &format!("empty; worktrees resolve to {}", resolved.display()),
                );
                true
            }
            Err(reason) => {
                print_check(
                    CheckStatus::Fail,
                    "worktree_base",
                    &format!(
                        "default {} not writable: {}; set defaults.worktree_base to a writable absolute directory",
                        resolved.display(),
                        reason
                    ),
                );
                false
            }
        };
    }
    let configured = Path::new(defaults.worktree_base.trim());
    if !configured.is_absolute() {
        // A relative or `~` path resolves against a different cwd during
        // dispatch than during doctor; require an absolute path so the
        // planned base is unambiguous.
        print_check(
            CheckStatus::Fail,
            "worktree_base",
            &format!(
                "not an absolute path: {}; set defaults.worktree_base to an absolute directory",
                configured.display()
            ),
        );
        return false;
    }
    match worktree_base_probe(configured) {
        Ok(()) => {
            print_check(
                CheckStatus::Pass,
                "worktree_base",
                &configured.display().to_string(),
            );
            true
        }
        Err(reason) => {
            print_check(
                CheckStatus::Fail,
                "worktree_base",
                &format!("not writable {}: {}", configured.display(), reason),
            );
            false
        }
    }
}

/// Probes that GAH could write under `path` without creating any directory
/// itself: the probe lands inside `path` when it exists, otherwise inside
/// the nearest existing ancestor. A polled `doctor --json` must not grow
/// directories under `$HOME` (or `/root` when `HOME` is unset) on every
/// run, so this never `create_dir_all`s the configured base.
fn worktree_base_probe(path: &Path) -> Result<(), String> {
    let probe_dir = if path.exists() {
        // The configured base must be a directory: a regular file here is a
        // hard failure, never something the ancestor probe papers over.
        if !path.is_dir() {
            return Err(format!("{} exists and is not a directory", path.display()));
        }
        path.to_path_buf()
    } else {
        path.ancestors()
            .skip(1)
            .find(|ancestor| ancestor.is_dir())
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| format!("no existing ancestor for {}", path.display()))?
    };
    let probe = probe_dir.join(".gah-write-test");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => Err(format!("{err}")),
    }
}

fn check_profile(defaults: &Defaults, profile: &Profile) -> bool {
    let mut failed = false;
    failed |= !check_repo(profile);
    failed |= !check_provider_cli(profile);
    failed |= !check_provider_auth(profile);
    failed |= !check_push_url(profile);
    failed |= !check_writable_path("artifact_root", Path::new(&profile.artifact_root));
    failed |= !check_manager_memory(defaults, profile);
    failed |= !check_candidate_model_consistency(defaults, profile);
    failed |= !check_backend_instance_config(defaults, profile);
    failed |= !check_generated_artifact_policy(profile);
    !failed
}

fn check_backend_instance_config(defaults: &Defaults, profile: &Profile) -> bool {
    match config::check_profile_backend_instances(defaults, profile) {
        Ok(()) => {
            let summaries = crate::config_show::backend_instance_summaries(defaults, profile);
            let routing = profile.effective_routing(defaults);
            let count = summaries.len();
            let mut ready = true;
            print_check(
                CheckStatus::Pass,
                "backend instances",
                &format!("{count} normalized instance declaration(s) valid"),
            );
            for instance in summaries {
                let status = if instance.executable_resolved {
                    CheckStatus::Pass
                } else if instance.enabled {
                    CheckStatus::Fail
                } else {
                    CheckStatus::Warn
                };
                print_check(
                    status,
                    "backend instance executable",
                    &format!(
                        "{}: {} ({})",
                        instance.backend_instance,
                        instance.resolution_error.as_deref().unwrap_or("resolved"),
                        instance.resolution_source
                    ),
                );
                if let Some(auth_ready) = routing
                    .backend_instances
                    .get(&instance.backend_instance)
                    .and_then(config::backend_instance_auth_ready)
                {
                    let auth_status = if auth_ready {
                        CheckStatus::Pass
                    } else if instance.enabled {
                        ready = false;
                        CheckStatus::Fail
                    } else {
                        CheckStatus::Warn
                    };
                    print_check(
                        auth_status,
                        "backend instance auth",
                        &format!(
                            "{}: {}",
                            instance.backend_instance,
                            if auth_ready {
                                "authenticated"
                            } else {
                                "provider CLI login required"
                            }
                        ),
                    );
                }
            }
            ready
        }
        Err(errors) => {
            for error in &errors {
                print_check(CheckStatus::Fail, "backend instances", error);
            }
            false
        }
    }
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
    let result = match profile.provider_kind() {
        Ok(crate::provider_kind::ProviderKind::Gitlab) => gitlab_provider_auth(profile),
        Ok(crate::provider_kind::ProviderKind::Github) => github_provider_auth(profile),
        Err(_) => {
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
    run_provider_cli_preflight(
        "gh",
        host,
        &format!("repos/{}", profile.repo),
        "no github token env var set",
    )
}

/// GitLab adapter: require a successful `glab api` preflight against the
/// exact configured GitLab host and project.
///
/// GitLab runtime paths use the host-scoped `glab` session directly, so a bare
/// `GITLAB_PAT`/`GITLAB_PAT2` value is not enough for `doctor` to report PASS.
fn gitlab_provider_auth(profile: &Profile) -> ProviderAuthResult {
    let Some(host) = gitlab_host(profile) else {
        return ProviderAuthResult::Failed(ProviderAuthFailure::HostUnconfigured(
            "gitlab profile missing provider_api_base; cannot determine the exact host to \
             authenticate against"
                .into(),
        ));
    };
    let Some(project_ref) = profile.provider_project_id.as_deref() else {
        return ProviderAuthResult::Failed(ProviderAuthFailure::HostUnconfigured(
            "gitlab profile missing provider_project_id; cannot determine the exact project to \
             authenticate against"
                .into(),
        ));
    };
    run_provider_cli_preflight(
        "glab",
        &host,
        &format!("/projects/{}", project_ref),
        "no authenticated glab session available",
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
    no_credential_detail: &str,
) -> ProviderAuthResult {
    if !which(cli) {
        return ProviderAuthResult::Failed(ProviderAuthFailure::NoCredential(format!(
            "{cli} not found on PATH and {no_credential_detail}"
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

fn check_manager_memory(defaults: &Defaults, profile: &Profile) -> bool {
    let node = match crate::node_role::NodeRoleStatus::resolve(defaults) {
        Ok(node) => node,
        Err(error) => {
            print_check(CheckStatus::Fail, "node role", &format!("{error:#}"));
            return false;
        }
    };
    // Worker execution uses central memory; it must not require a local manager store.
    if node.role == crate::node_role::NodeRole::Worker {
        print_check(
            CheckStatus::Pass,
            "manager memory",
            "managed by central; no local manager memory required",
        );
        return true;
    }
    let path = Path::new(&profile.local_path).join("docs/MANAGER_MEMORY.md");
    if path.exists() {
        print_check(
            CheckStatus::Pass,
            "manager memory",
            &path.display().to_string(),
        );
        true
    } else {
        // The file is the manager's optional standing notes. Dispatch, review
        // and the loop run without it, so a new profile must not fail here.
        print_check(
            CheckStatus::Warn,
            "manager memory",
            &format!(
                "missing {} (optional: the manager starts without standing project notes)",
                path.display()
            ),
        );
        true
    }
}

/// TICKET-076: `--validate` extends the existing checks with execution
/// prerequisites doctor doesn't already cover -- whether the configured
/// validation commands and backend executables actually resolve, and
/// whether declared env files exist. Deliberately does not re-check repo
/// path, provider CLI/token, push URL, or writable roots -- `check_profile`
/// and `check_worktree_base` already cover those.
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
    if *policy == config::MergePolicy::GitlabMwps
        && profile.provider_kind() != Ok(crate::provider_kind::ProviderKind::Gitlab)
    {
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

fn parse_agy_models(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // `agy models` prints the selectable label after the tab.
            let label = line.split_once('\t')?.1.trim();
            (!label.is_empty()).then(|| label.to_string())
        })
        .collect()
}

fn check_candidate_models(defaults: &Defaults, profile: &Profile) -> bool {
    let routing = profile.effective_routing(defaults);
    let mut candidates = Vec::new();
    if let Some(candidate) = &routing.routine_reviewer {
        candidates.push(candidate);
    }
    candidates.extend(&routing.escalatory_reviewers);
    for list in [
        &routing.pm_candidates,
        &routing.improve_candidates,
        &routing.review_candidates,
    ]
    .into_iter()
    .flatten()
    {
        candidates.extend(list);
    }
    for rule in &routing.task_routing_rules {
        candidates.extend(&rule.candidates);
    }
    let mut cache = std::collections::HashMap::new();
    let mut valid = true;
    for candidate in candidates {
        let Some(model) = candidate.model.as_deref() else {
            continue;
        };
        let instance = candidate
            .instance
            .as_deref()
            .and_then(|name| routing.backend_instances.get(name));
        let runner_kind = instance
            .map(|entry| entry.runner_kind.as_str())
            .unwrap_or(&candidate.backend);
        if runner_kind != "agy" {
            continue;
        }
        let resolution = if let Some(instance) = instance {
            crate::runner::resolve_backend_instance_executable(instance)
        } else {
            crate::runner::resolve_backend_executable(profile, &candidate.backend)
        };
        let crate::runner::ExecutableResolution::Found(path) = resolution else {
            // The executable check reports this separately.
            continue;
        };
        // Dispatch launches each instance with its isolated account state, so
        // the model list must come from that same account.
        let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            runner_kind,
            None::<String>,
            None::<String>,
        );
        identity.state_root = instance
            .and_then(|entry| entry.state_root.as_deref())
            .filter(|root| !root.is_empty())
            .map(std::path::PathBuf::from);
        let mut state_env = Vec::new();
        identity.apply_instance_state_env(&mut state_env);
        let cache_key = (path.clone(), identity.state_root.clone());
        let models = cache.entry(cache_key).or_insert_with(|| {
            Command::new(&path)
                .arg("models")
                .envs(state_env)
                .output()
                .map(|result| {
                    if !result.status.success() {
                        return Err(format!(
                            "agy models exited {}: {}",
                            result.status,
                            String::from_utf8_lossy(&result.stderr).trim()
                        ));
                    }
                    let models = parse_agy_models(&String::from_utf8_lossy(&result.stdout));
                    if models.is_empty() {
                        Err("agy models returned no selectable models".to_string())
                    } else {
                        Ok(models)
                    }
                })
                .unwrap_or_else(|error| Err(format!("cannot run agy models: {error}")))
        });
        let label = format!(
            "{}/{}",
            candidate.instance.as_deref().unwrap_or(&candidate.backend),
            model
        );
        match models {
            Ok(models) if models.iter().any(|available| available == model) => {
                print_check(
                    CheckStatus::Pass,
                    "candidate model",
                    &format!("{label}: accepted by agy"),
                );
            }
            Ok(models) => {
                print_check(
                    CheckStatus::Fail,
                    "candidate model",
                    &format!(
                        "{label}: invalid model; available models: {}",
                        models.join(", ")
                    ),
                );
                valid = false;
            }
            Err(error) => {
                print_check(
                    CheckStatus::Fail,
                    "candidate model",
                    &format!("{label}: {error}"),
                );
                valid = false;
            }
        }
    }
    valid
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn print_check(status: CheckStatus, label: &str, detail: &str) {
    let captured = CHECK_CAPTURE.with(|capture| {
        let mut capture = capture.borrow_mut();
        let Some(capture) = capture.as_mut() else {
            return false;
        };
        capture.checks.push(DoctorCheck {
            profile: capture.profile.clone(),
            name: label.to_string(),
            status: status.json_str().to_string(),
            detail: detail.to_string(),
        });
        true
    });
    if !captured {
        println!("[{}] {:<16} {}", status.as_str(), label, detail);
    }
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

    fn json_str(self) -> &'static str {
        match self {
            Self::Pass => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{check_push_url, gitlab_provider_auth, ProviderAuthFailure, ProviderAuthResult};
    use crate::config::{Profile, RoutingPolicy};

    #[test]
    fn agy_model_list_rejects_unknown_candidate() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("agy");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'gemini-id\\tGemini 3.7 Flash (High)\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let mut profile = gitlab_profile(None);
        profile.agy_path = Some(executable.display().to_string());
        profile.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
            backend: "agy".into(),
            model: Some("default".into()),
            ..Default::default()
        }]);
        assert!(!super::check_candidate_models(
            &crate::config::Defaults::default(),
            &profile
        ));
        profile.routing.pm_candidates.as_mut().unwrap()[0].model =
            Some("Gemini 3.7 Flash (High)".into());
        assert!(super::check_candidate_models(
            &crate::config::Defaults::default(),
            &profile
        ));
    }

    #[test]
    fn agy_model_list_uses_each_instance_state_root() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("agy");
        // Each isolated account exposes only the model named in its HOME.
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'id\\t%s\\n' \"$(cat \"$HOME/model\")\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let mut profile = gitlab_profile(None);
        for (name, model) in [("agy-a", "Model A"), ("agy-b", "Model B")] {
            let root = temp.path().join(name);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("model"), model).unwrap();
            profile.routing.backend_instances.insert(
                name.into(),
                crate::config::BackendInstanceConfig {
                    runner_kind: "agy".into(),
                    enabled: None,
                    logical_backend: None,
                    executable: Some(executable.display().to_string()),
                    resolve_from_path: None,
                    state_root: Some(root.display().to_string()),
                    account_label: None,
                    auth_source_label: None,
                    credential_id: None,
                    quota_pool: None,
                    supported_models: Vec::new(),
                },
            );
        }
        let candidate = |instance: &str, model: &str| crate::config::CandidateConfig {
            backend: "agy".into(),
            instance: Some(instance.into()),
            model: Some(model.into()),
            ..Default::default()
        };
        profile.routing.pm_candidates = Some(vec![
            candidate("agy-a", "Model A"),
            candidate("agy-b", "Model B"),
        ]);
        assert!(super::check_candidate_models(
            &crate::config::Defaults::default(),
            &profile
        ));
        // A model only the other account offers must not be accepted.
        profile.routing.pm_candidates = Some(vec![candidate("agy-b", "Model A")]);
        assert!(!super::check_candidate_models(
            &crate::config::Defaults::default(),
            &profile
        ));
    }

    fn gitlab_profile(api_base: Option<&str>) -> Profile {
        Profile {
            delivery_mode: crate::config::DeliveryMode::default(),
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            prune_older_than_days: None,
            chat_session_idle_days: None,
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
            hermes_args: vec![],
            hermes_path: None,
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
            hermes_idle_timeout_seconds: None,
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
            external_credential_scopes: std::collections::HashMap::new(),
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

    // Issue #1366: the worktree base is a defaults-level concern, so doctor
    // must validate it even with no profiles. An empty value is tolerated
    // (work GAH plans resolves the default at the point of use) and must
    // resolve instead of failing; doctor must still reject an unwritable,
    // non-absolute, or file-valued base, and must never create the base.
    #[test]
    fn doctor_worktree_base_check_resolves_empty_to_default() {
        let defaults = crate::config::Defaults::default();

        assert!(super::check_worktree_base(&defaults));
    }

    #[test]
    fn doctor_worktree_base_check_fails_when_unwritable() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_directory = tmp.path().join("worktree-base-is-a-file");
        std::fs::write(&not_a_directory, "regular file").unwrap();
        let defaults = crate::config::Defaults {
            worktree_base: not_a_directory.display().to_string(),
            ..Default::default()
        };

        assert!(!super::check_worktree_base(&defaults));
    }

    #[test]
    fn doctor_worktree_base_check_fails_when_relative() {
        let defaults = crate::config::Defaults {
            worktree_base: "worktrees/relative".to_string(),
            ..Default::default()
        };

        assert!(!super::check_worktree_base(&defaults));
        let tilde = crate::config::Defaults {
            worktree_base: "~/worktrees".to_string(),
            ..Default::default()
        };
        assert!(!super::check_worktree_base(&tilde));
    }

    #[test]
    fn doctor_worktree_base_probe_creates_no_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("a/b/c");
        let defaults = crate::config::Defaults {
            worktree_base: base.display().to_string(),
            ..Default::default()
        };

        assert!(super::check_worktree_base(&defaults));
        // The probe must verify writability without materializing the base:
        // a polled doctor would otherwise grow directories on every run.
        assert!(!base.exists());
        assert!(!tmp.path().join("a").exists());
    }

    #[test]
    fn doctor_gitlab_preflight_requires_provider_project_id() {
        let mut profile = gitlab_profile(Some("https://gitlab.example.internal/api/v4"));
        profile.provider_project_id = None;

        match gitlab_provider_auth(&profile) {
            ProviderAuthResult::Failed(ProviderAuthFailure::HostUnconfigured(message)) => {
                assert!(message.contains("provider_project_id"));
            }
            _ => panic!("expected provider_project_id to be required"),
        }
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
