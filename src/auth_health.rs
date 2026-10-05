//! Issue #1271: one answer to "is this login still good?" for every backend
//! and provider on this node.
//!
//! The classifiers here are the single place that reads provider CLI login
//! output; the manager discovery code and backend-instance readiness call into
//! them. `probe_node` runs the bounded, secret-free checks behind
//! `gah auth-health`, and adds logins that dispatch already found dead at
//! runtime (availability records with an authentication reason).
//!
//! No probe prints, logs, or returns provider output: details are fixed
//! strings chosen here, so a token echoed by a CLI can never leave the node.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Ok,
    Expired,
    Missing,
    Unknown,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthHealth {
    pub state: AuthState,
    /// A fixed, secret-free explanation; never provider output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AuthHealth {
    fn new(state: AuthState, detail: Option<&str>) -> Self {
        Self {
            state,
            detail: detail.map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    /// A login check run by `gah auth-health`.
    Probe,
    /// A dispatch attempt failed with an authentication signature.
    Dispatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthProbe {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    /// The provider behind a multi-provider backend (opencode), or the API
    /// provider for a key-only login. `None` for single-login backends.
    pub provider: Option<String>,
    #[serde(flatten)]
    pub health: AuthHealth,
    pub source: AuthSource,
    /// Repository package presence is independent of its login state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthHealthReport {
    pub checked_at: String,
    pub probes: Vec<AuthProbe>,
}

fn said_logged_in(lower: &str) -> bool {
    lower.lines().any(|line| {
        (line.contains("logged in") && !line.contains("not logged in"))
            || (line.contains("authenticated")
                && !line.contains("not authenticated")
                && !line.contains("unauthenticated"))
    })
}

fn said_expired(lower: &str) -> bool {
    [
        "expired",
        "token is invalid",
        "invalid token",
        "token has been revoked",
        "revoked",
        "re-authenticate",
        "reauthenticate",
        "failed to log in",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn said_logged_out(lower: &str) -> bool {
    [
        "not logged in",
        "logged out",
        "no credentials",
        "not authenticated",
        "unauthenticated",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Classifies a text `login status`-style command (codex, hermes, gh, glab).
/// Expiry is checked first: gh reports a dead token next to its account line.
pub fn classify_status_output(success: bool, stdout: &[u8], stderr: &[u8]) -> AuthHealth {
    let lower = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_lowercase();
    if said_expired(&lower) {
        return AuthHealth::new(AuthState::Expired, Some("The saved login has expired."));
    }
    if said_logged_out(&lower) {
        return AuthHealth::new(AuthState::Missing, Some("Not logged in."));
    }
    if success && said_logged_in(&lower) {
        return AuthHealth::new(AuthState::Ok, None);
    }
    if success {
        AuthHealth::new(
            AuthState::Unknown,
            Some("The login status was not recognized."),
        )
    } else {
        AuthHealth::new(AuthState::Error, Some("The login status command failed."))
    }
}

/// Classifies `claude auth status --json`.
pub fn classify_claude_status(success: bool, stdout: &[u8]) -> AuthHealth {
    let logged_in = serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|status| status.get("loggedIn").and_then(|value| value.as_bool()));
    match logged_in {
        Some(true) => AuthHealth::new(AuthState::Ok, None),
        Some(false) => AuthHealth::new(AuthState::Missing, Some("Not logged in.")),
        None if success => AuthHealth::new(
            AuthState::Unknown,
            Some("The login status was not recognized."),
        ),
        None => AuthHealth::new(AuthState::Error, Some("The login status command failed.")),
    }
}

/// Classifies an authenticated HTTP request made with an API key.
pub fn classify_http_status(status: u16) -> AuthHealth {
    match status {
        200..=299 => AuthHealth::new(AuthState::Ok, None),
        401 | 403 => AuthHealth::new(
            AuthState::Expired,
            Some("The API key was rejected (HTTP 401/403)."),
        ),
        _ => AuthHealth::new(
            AuthState::Error,
            Some("The provider did not answer the key check."),
        ),
    }
}

/// opencode names providers by display name in `auth list` ("GitHub
/// Copilot") and by id in `models` ("github-copilot/gpt-4o").
fn opencode_provider_id(display: &str) -> String {
    display
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Providers with a saved credential in `opencode auth list` output.
fn opencode_credential_providers(auth_list: &str) -> Vec<String> {
    let mut providers = Vec::new();
    for line in auth_list.lines() {
        // Entries render as "●  GitHub Copilot oauth" (or "api"/"wellknown").
        let Some(entry) = line.trim().strip_prefix('●') else {
            continue;
        };
        let mut words: Vec<&str> = entry.split_whitespace().collect();
        if words.len() < 2 {
            continue;
        }
        words.pop();
        let id = opencode_provider_id(&words.join(" "));
        if !providers.contains(&id) {
            providers.push(id);
        }
    }
    providers
}

/// A saved credential without a model catalog has unknown login validity:
/// model discovery alone cannot establish that the credential expired.
pub fn classify_opencode(auth_list: &str, models: &str) -> Vec<(String, AuthHealth)> {
    opencode_credential_providers(auth_list)
        .into_iter()
        .map(|provider| {
            let prefix = format!("{provider}/");
            let listed = models
                .lines()
                .any(|line| line.trim().to_lowercase().starts_with(&prefix));
            let health = if listed {
                AuthHealth::new(AuthState::Ok, None)
            } else {
                AuthHealth::new(
                    AuthState::Unknown,
                    Some("No models were listed for this provider. Login validity is unknown."),
                )
            };
            (provider, health)
        })
        .collect()
}

/// Whether a failed chat turn or dispatch attempt failed to authenticate.
/// Uses the dispatch failure parser's markers so there is one list.
pub fn is_auth_failure(backend: &str, text: &str) -> bool {
    crate::quota_parser::parse(backend, text, time::OffsetDateTime::now_utc()).is_some_and(
        |failure| failure.kind == crate::quota_parser::FailureKind::AuthenticationError,
    )
}

fn run(executable: &Path, args: &[&str], env: &[(&str, &Path)]) -> Option<std::process::Output> {
    let mut command = Command::new(executable);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    crate::runner::process::run_bounded(command, PROBE_TIMEOUT)
}

fn timed_out() -> AuthHealth {
    AuthHealth::new(AuthState::Error, Some("The login check did not finish."))
}

/// Codex login state from `codex login status`, optionally under an
/// isolated state root. `None` when the check did not finish.
pub fn codex_login(executable: &Path, state_root: Option<&Path>) -> Option<AuthHealth> {
    let codex_home = state_root.map(|root| root.join(".codex"));
    let mut env: Vec<(&str, &Path)> = Vec::new();
    if let (Some(root), Some(home)) = (state_root, codex_home.as_deref()) {
        env.push(("HOME", root));
        env.push(("CODEX_HOME", home));
    }
    run(executable, &["login", "status"], &env).map(|output| {
        classify_status_output(output.status.success(), &output.stdout, &output.stderr)
    })
}

/// Claude login state from `claude auth status --json`. `None` when the
/// check did not finish.
pub fn claude_login(executable: &Path, state_root: Option<&Path>) -> Option<AuthHealth> {
    let config_dir = state_root.map(|root| root.join(".claude"));
    let mut env: Vec<(&str, &Path)> = Vec::new();
    if let (Some(root), Some(dir)) = (state_root, config_dir.as_deref()) {
        env.push(("HOME", root));
        env.push(("CLAUDE_CONFIG_DIR", dir));
    }
    run(executable, &["auth", "status", "--json"], &env)
        .map(|output| classify_claude_status(output.status.success(), &output.stdout))
}

fn resolve(command: &str) -> Option<PathBuf> {
    crate::runner::resolve::resolve_executable_on_path(command)
}

fn probe(backend: &str, provider: Option<&str>, health: AuthHealth) -> AuthProbe {
    AuthProbe {
        backend: backend.to_string(),
        backend_instance: None,
        provider: provider.map(str::to_string),
        health,
        source: AuthSource::Probe,
        installed: None,
    }
}

/// Report missing packages separately from an installed CLI that needs login.
fn repository_probe(cli: &str, provider: &str, executable: Option<&Path>) -> AuthProbe {
    let health = match executable {
        Some(executable) => run(executable, &["auth", "status"], &[])
            .map(|output| {
                classify_status_output(output.status.success(), &output.stdout, &output.stderr)
            })
            .unwrap_or_else(timed_out),
        None => AuthHealth::new(
            AuthState::Missing,
            Some("Install the repository CLI package before signing in."),
        ),
    };
    let mut result = probe(cli, Some(provider), health);
    result.installed = Some(executable.is_some());
    result
}

/// The HTTP status of one authenticated GET. The key reaches curl through a
/// config on stdin, never argv. Unlike coordinator requests, no TLS override
/// applies: a provider key is only sent over verified TLS.
fn bearer_status(url: &str, key: &str) -> Option<u16> {
    use std::io::Write;
    use std::process::Stdio;
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--proto",
            "=https",
            "--max-time",
            "10",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-K",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()?
        .write_all(
            format!(
                "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\n",
                quote(url),
                quote(key)
            )
            .as_bytes(),
        )
        .ok()?;
    let output = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn api_key_probe(provider: &str, env_var: &str, models_url: &str) -> Option<AuthProbe> {
    let key = std::env::var(env_var)
        .ok()
        .filter(|key| !key.trim().is_empty() && !key.contains(['\n', '\r']))?;
    let health = match bearer_status(models_url, key.trim()) {
        Some(status) if status > 0 => classify_http_status(status),
        _ => AuthHealth::new(
            AuthState::Error,
            Some("The provider did not answer the key check."),
        ),
    };
    Some(probe("api", Some(provider), health))
}

/// Logins dispatch found dead: availability records that still block a
/// backend for an authentication reason.
fn dispatch_failures(now: time::OffsetDateTime) -> Vec<AuthProbe> {
    let Ok(scopes) =
        crate::availability::list_scopes(&crate::availability::resolve_state_path(), now)
    else {
        return Vec::new();
    };
    let mut failures: Vec<AuthProbe> = Vec::new();
    for scope in scopes {
        if scope.eligible || scope.reason != Some(crate::availability::Reason::AuthenticationError)
        {
            continue;
        }
        if failures
            .iter()
            .any(|failure| failure.backend == scope.backend)
        {
            continue;
        }
        failures.push(AuthProbe {
            backend: scope.backend,
            backend_instance: None,
            provider: None,
            health: AuthHealth::new(
                AuthState::Expired,
                Some("A dispatch attempt failed to authenticate."),
            ),
            source: AuthSource::Dispatch,
            installed: None,
        });
    }
    failures
}

fn subscription_probes(config: &crate::config::GahConfig) -> Vec<AuthProbe> {
    subscription_probes_with(config, |instance| {
        let info = crate::credentials::get(instance.credential_id.as_deref()?).ok()?;
        (info.kind == crate::credentials::CredentialKind::ClaudeSubscription)
            .then(|| crate::config::backend_instance_auth_ready(instance).unwrap_or(false))
    })
}

fn subscription_probes_with(
    config: &crate::config::GahConfig,
    mut readiness: impl FnMut(&crate::config::BackendInstanceConfig) -> Option<bool>,
) -> Vec<AuthProbe> {
    let mut probes = Vec::new();

    for profile in config.profiles.values() {
        for (id, instance) in &profile
            .effective_routing(&config.defaults)
            .backend_instances
        {
            if instance.runner_kind != "claude" || !instance.enabled() {
                continue;
            }
            let Some(ready) = readiness(instance) else {
                continue;
            };
            let mut result = probe(
                "claude",
                None,
                AuthHealth::new(
                    if ready {
                        AuthState::Ok
                    } else {
                        AuthState::Missing
                    },
                    Some(if ready {
                        "Claude subscription token is configured."
                    } else {
                        "Claude subscription instance is unavailable."
                    }),
                ),
            );
            result.backend_instance = Some(id.clone());
            probes.push(result);
        }
    }
    probes
}

/// Every login this node can check, plus logins dispatch found dead.
/// Backends whose CLI is not installed are skipped, not reported missing.
pub fn probe_node() -> AuthHealthReport {
    let now = time::OffsetDateTime::now_utc();
    let mut probes = Vec::new();
    if let Ok(config) = crate::config::load(None) {
        probes.extend(subscription_probes(&config));
    }
    if let Some(claude) = resolve("claude") {
        probes.push(probe(
            "claude",
            None,
            claude_login(&claude, None).unwrap_or_else(timed_out),
        ));
    }
    if let Some(codex) = resolve("codex") {
        probes.push(probe(
            "codex",
            None,
            codex_login(&codex, None).unwrap_or_else(timed_out),
        ));
    }
    if let Some(hermes) = resolve("hermes") {
        let health = run(&hermes, &["status", "--all"], &[])
            .map(|output| {
                classify_status_output(output.status.success(), &output.stdout, &output.stderr)
            })
            .unwrap_or_else(timed_out);
        probes.push(probe("hermes", None, health));
    }
    if let Some(opencode) = resolve("opencode") {
        let auth = run(&opencode, &["auth", "list"], &[]);
        let models = run(&opencode, &["models"], &[]);
        match (auth, models) {
            (Some(auth), Some(models)) if auth.status.success() && models.status.success() => {
                for (provider, health) in classify_opencode(
                    &String::from_utf8_lossy(&auth.stdout),
                    &String::from_utf8_lossy(&models.stdout),
                ) {
                    probes.push(probe("opencode", Some(&provider), health));
                }
            }
            _ => probes.push(probe(
                "opencode",
                None,
                AuthHealth::new(AuthState::Error, Some("The login check did not finish.")),
            )),
        }
    }
    let configured_providers: std::collections::HashSet<String> = crate::config::load(None)
        .map(|config| {
            config
                .profiles
                .values()
                .map(|profile| profile.provider.clone())
                .collect()
        })
        .unwrap_or_default();
    for (cli, provider) in [("gh", "github"), ("glab", "gitlab")] {
        let executable = resolve(cli);
        if executable.is_some() || configured_providers.contains(provider) {
            probes.push(repository_probe(cli, provider, executable.as_deref()));
        }
    }
    probes.extend(api_key_probe(
        "mistral",
        "MISTRAL_API_KEY",
        "https://api.mistral.ai/v1/models",
    ));
    probes.extend(api_key_probe(
        "nous",
        "NOUS_API_KEY",
        "https://inference-api.nousresearch.com/v1/models",
    ));
    probes.extend(dispatch_failures(now));
    AuthHealthReport {
        checked_at: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        probes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_health_uses_inherited_instances_and_runner_kind() {
        let mut config = crate::config::GahConfig {
            defaults: Default::default(),
            profiles: Default::default(),
            context: Default::default(),
        };
        let mut profile = crate::config::tests::test_profile_for_notifications();
        for id in ["global", "overridden", "disabled"] {
            config.defaults.routing.backend_instances.insert(
                id.into(),
                crate::config::BackendInstanceConfig {
                    runner_kind: "claude".into(),
                    credential_id: Some("global-token".into()),
                    ..Default::default()
                },
            );
        }
        profile.routing.backend_instances.insert(
            "overridden".into(),
            crate::config::BackendInstanceConfig {
                credential_id: Some("profile-token".into()),
                ..Default::default()
            },
        );
        profile.routing.backend_instances.insert(
            "disabled".into(),
            crate::config::BackendInstanceConfig {
                enabled: Some(false),
                ..Default::default()
            },
        );
        config.profiles.insert("test".into(), profile);
        let probes = subscription_probes_with(&config, |instance| {
            assert_eq!(instance.runner_kind, "claude");
            Some(instance.credential_id.as_deref() == Some("global-token"))
        });
        assert_eq!(probes.len(), 2);
        assert!(probes
            .iter()
            .any(|probe| probe.backend_instance.as_deref() == Some("global")
                && probe.health.state == AuthState::Ok));
        assert!(probes.iter().any(
            |probe| probe.backend_instance.as_deref() == Some("overridden")
                && probe.health.state == AuthState::Missing
        ));
    }

    fn text(success: bool, stdout: &str) -> AuthState {
        classify_status_output(success, stdout.as_bytes(), b"").state
    }

    #[test]
    fn missing_repository_package_is_not_a_login_failure() {
        let probe = repository_probe("gh", "github", None);
        assert_eq!(probe.installed, Some(false));
        assert_eq!(probe.health.state, AuthState::Missing);
        assert!(probe
            .health
            .detail
            .unwrap()
            .contains("package before signing in"));
    }

    #[test]
    fn codex_login_status_output() {
        assert_eq!(text(true, "Logged in using ChatGPT\n"), AuthState::Ok);
        assert_eq!(text(true, "OpenAI Codex ✓ logged in\n"), AuthState::Ok);
        assert_eq!(text(false, "Not logged in\n"), AuthState::Missing);
        assert_eq!(
            text(false, "Your access token could not be refreshed because your refresh token has expired.\n"),
            AuthState::Expired
        );
        assert_eq!(text(true, "Mystery status\n"), AuthState::Unknown);
        assert_eq!(text(false, ""), AuthState::Error);
    }

    #[test]
    fn gh_auth_status_output() {
        assert_eq!(
            text(true, "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n"),
            AuthState::Ok
        );
        assert_eq!(
            text(
                false,
                "github.com\n  X Failed to log in to github.com account octo (default)\n  - The token in default is invalid.\n"
            ),
            AuthState::Expired
        );
        assert_eq!(
            text(
                false,
                "You are not logged into any GitHub hosts. To log in, run: gh auth login\n"
            ),
            AuthState::Missing
        );
    }

    #[test]
    fn hermes_status_output() {
        assert_eq!(
            text(true, "Provider: nous — authenticated\n"),
            AuthState::Ok
        );
        assert_eq!(
            text(true, "Provider: nous — not authenticated\n"),
            AuthState::Missing
        );
    }

    #[test]
    fn claude_status_json() {
        let state =
            |success: bool, body: &str| classify_claude_status(success, body.as_bytes()).state;
        assert_eq!(
            state(true, r#"{"loggedIn":true,"authMethod":"claude.ai"}"#),
            AuthState::Ok
        );
        assert_eq!(state(false, r#"{"loggedIn":false}"#), AuthState::Missing);
        assert_eq!(state(true, "not json"), AuthState::Unknown);
        assert_eq!(state(false, "not json"), AuthState::Error);
    }

    #[test]
    fn opencode_credential_without_models_has_unknown_login_validity() {
        let auth_list = "┌  Credentials ~/.local/share/opencode/auth.json\n│\n●  GitHub Copilot oauth\n│\n●  Anthropic api\n│\n└  2 credentials\n";
        let models = "anthropic/claude-sonnet-4\nanthropic/claude-opus-4\nopencode/big-pickle\n";
        let results = classify_opencode(auth_list, models);
        assert_eq!(
            results
                .iter()
                .map(|(provider, health)| (provider.as_str(), health.state))
                .collect::<Vec<_>>(),
            vec![
                ("github-copilot", AuthState::Unknown),
                ("anthropic", AuthState::Ok)
            ]
        );
        assert_eq!(
            results[0].1.detail.as_deref(),
            Some("No models were listed for this provider. Login validity is unknown.")
        );
    }

    #[test]
    fn api_key_http_status() {
        assert_eq!(classify_http_status(200).state, AuthState::Ok);
        assert_eq!(classify_http_status(401).state, AuthState::Expired);
        assert_eq!(classify_http_status(503).state, AuthState::Error);
    }

    #[test]
    fn runtime_failures_use_the_dispatch_markers() {
        assert!(is_auth_failure("vibe", "Error: invalid API key provided"));
        assert!(is_auth_failure("agy", "Error: not logged into Antigravity"));
        assert!(is_auth_failure("opencode", "Error: HTTP 401 Unauthorized"));
        assert!(is_auth_failure(
            "codex",
            "error: 403 Forbidden from the model API"
        ));
        assert!(is_auth_failure(
            "claude",
            "OAuth token has expired. Please run /login."
        ));
        assert!(is_auth_failure(
            "claude",
            "Not logged in · Please run /login"
        ));
        assert!(!is_auth_failure("claude", "The tests failed on line 401."));
        assert!(!is_auth_failure("claude", "Fixed issue #403 and pushed."));
    }

    #[test]
    fn details_are_fixed_strings_not_provider_output() {
        let health = classify_status_output(false, b"token ghp_secret123 has expired", b"");
        assert_eq!(
            health.detail.as_deref(),
            Some("The saved login has expired.")
        );
    }
}
