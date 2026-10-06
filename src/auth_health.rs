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
        "401 unauthorized",
        "bad credentials",
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

/// Classifies `gh auth status` for one host. gh lists every saved account
/// of every host, so only the host's active account decides (#1324): a
/// dead token on an inactive account or another host must not hide a
/// fresh login. Output without that host falls back to the whole text.
pub fn classify_gh_status(host: &str, success: bool, stdout: &[u8], stderr: &[u8]) -> AuthHealth {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    // Host headers are unindented; account entries are indented lines
    // starting with a ✓, X or ! mark, followed by their `- ` details.
    let mut section: Option<Vec<&str>> = None;
    let mut entries: Vec<Vec<&str>> = Vec::new();
    let mut current_host = "";
    for line in text.lines() {
        if !line.is_empty() && !line.starts_with(char::is_whitespace) {
            current_host = line.trim();
            continue;
        }
        if current_host != host {
            continue;
        }
        section.get_or_insert_with(Vec::new).push(line);
        let trimmed = line.trim_start();
        if ["✓ ", "X ", "! "]
            .iter()
            .any(|mark| trimmed.starts_with(mark))
        {
            entries.push(vec![line]);
        } else if let Some(entry) = entries.last_mut() {
            entry.push(line);
        }
    }
    let active = entries.iter().find(|entry| {
        entry
            .iter()
            .any(|line| line.trim().eq_ignore_ascii_case("- active account: true"))
    });
    match (active, section) {
        // The entry's own mark decides; the exit code covers every account.
        (Some(entry), _) => classify_status_output(true, entry.join("\n").as_bytes(), b""),
        (None, Some(section)) => {
            classify_status_output(success, section.join("\n").as_bytes(), b"")
        }
        (None, None) => classify_status_output(success, stdout, stderr),
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

pub(crate) fn timed_out() -> AuthHealth {
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

/// Whether this backend's own login check says it is signed in right now.
/// `false` when the backend has no such check, its executable is unknown, or
/// the check did not finish: only a positive answer may overrule a login
/// failure read from a backend's output.
pub fn login_confirmed(identity: &crate::execution_identity::ExecutionIdentity) -> bool {
    let Some(executable) = identity.executable.as_deref() else {
        return false;
    };
    let state_root = identity.state_root.as_deref();
    let health = match identity.runner_kind.as_str() {
        "codex" => codex_login(executable, state_root),
        "claude" => claude_login(executable, state_root),
        _ => None,
    };
    health.is_some_and(|health| health.state == AuthState::Ok)
}

fn resolve(command: &str) -> Option<PathBuf> {
    crate::runner::resolve::resolve_executable_on_path(command)
}

fn probe(backend: &str, provider: Option<&str>, health: AuthHealth) -> AuthProbe {
    AuthProbe {
        backend: backend.to_string(),
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
                if cli == "gh" {
                    classify_gh_status(
                        "github.com",
                        output.status.success(),
                        &output.stdout,
                        &output.stderr,
                    )
                } else {
                    classify_status_output(output.status.success(), &output.stdout, &output.stderr)
                }
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

/// Every login this node can check, plus logins dispatch found dead.
/// Backends whose CLI is not installed are skipped, not reported missing.
pub fn probe_node() -> AuthHealthReport {
    let now = time::OffsetDateTime::now_utc();
    let mut probes = Vec::new();
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

    fn text(success: bool, stdout: &str) -> AuthState {
        classify_status_output(success, stdout.as_bytes(), b"").state
    }

    fn gh(success: bool, stdout: &str, stderr: &str) -> AuthState {
        classify_gh_status("github.com", success, stdout.as_bytes(), stderr.as_bytes()).state
    }

    const GH_OK: &str = "  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n  - Git operations protocol: https\n  - Token: gho_************\n  - Token scopes: 'gist', 'read:org', 'repo'\n";

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

    /// The shared classifier keeps "logged out" ahead of a login line, so a
    /// listing with one logged-out provider never reads as logged in.
    #[test]
    fn a_logged_out_line_beats_a_login_line_outside_gh() {
        assert_eq!(
            text(true, "openai: ✓ logged in\nanthropic: not logged in\n"),
            AuthState::Missing
        );
    }

    /// glab reports a revoked token as an API 401 on a failed command.
    #[test]
    fn glab_401_is_a_rejected_credential() {
        assert_eq!(
            text(
                false,
                "gitlab.com\n  x gitlab.com: api call failed: GET https://gitlab.com/api/v4/user: 401 {message: 401 Unauthorized}\n"
            ),
            AuthState::Expired
        );
    }

    #[test]
    fn gh_auth_status_output() {
        assert_eq!(gh(true, &format!("github.com\n{GH_OK}"), ""), AuthState::Ok);
        assert_eq!(
            gh(
                false,
                "github.com\n  X Failed to log in to github.com account octo (default)\n  - The token in default is invalid.\n",
                ""
            ),
            AuthState::Expired
        );
        assert_eq!(
            gh(
                false,
                "",
                "You are not logged into any GitHub hosts. To log in, run: gh auth login\n"
            ),
            AuthState::Missing
        );
        assert_eq!(
            gh(
                true,
                &format!("github.com\n{GH_OK}\nYou are not logged into any GitHub Enterprise Server hosts.\n"),
                ""
            ),
            AuthState::Ok
        );
    }

    /// Issue #1324: gh 2.45.0 account states, reproduced with the real
    /// binary. A dead token is printed on stdout by a command that still
    /// exits 0; "not logged in" is a failed command whose message goes to
    /// stderr only; a validation timeout is neither Ok nor a rejection.
    #[test]
    fn gh_245_auth_status_account_states() {
        assert_eq!(gh(true, &format!("github.com\n{GH_OK}"), ""), AuthState::Ok);
        assert_eq!(
            gh(
                true,
                "github.com\n  X Failed to log in to github.com account octo (keyring)\n  - Active account: true\n  - The token in keyring is invalid.\n  - To re-authenticate, run: gh auth login -h github.com\n  - To forget about this account, run: gh auth logout -h github.com -u octo\n",
                ""
            ),
            AuthState::Expired
        );
        assert_eq!(
            gh(
                true,
                "github.com\n  X Timeout trying to log in to github.com account octo (keyring)\n  - Active account: true\n",
                ""
            ),
            AuthState::Unknown
        );
        assert_eq!(gh(false, "", ""), AuthState::Error);
    }

    /// Issue #1324: a fresh login next to an old account whose token is
    /// dead. Only the active account decides, in either order.
    #[test]
    fn gh_inactive_dead_account_does_not_hide_the_active_login() {
        let dead = "  X Failed to log in to github.com account old (keyring)\n  - Active account: false\n  - The token in keyring is invalid.\n  - To re-authenticate, run: gh auth login -h github.com\n";
        assert_eq!(
            gh(true, &format!("github.com\n{GH_OK}\n{dead}"), ""),
            AuthState::Ok
        );
        assert_eq!(
            gh(false, &format!("github.com\n{dead}\n{GH_OK}"), ""),
            AuthState::Ok
        );
        let dead_active = dead.replace("Active account: false", "Active account: true");
        let ok_inactive = GH_OK.replace("Active account: true", "Active account: false");
        assert_eq!(
            gh(
                true,
                &format!("github.com\n{ok_inactive}\n{dead_active}"),
                ""
            ),
            AuthState::Expired
        );
    }

    /// Another host's dead token says nothing about github.com.
    #[test]
    fn gh_other_host_failure_does_not_hide_the_login() {
        assert_eq!(
            gh(
                true,
                &format!("github.com\n{GH_OK}\nghe.example.com\n  X Failed to log in to ghe.example.com account octo (keyring)\n  - Active account: true\n  - The token in keyring is invalid.\n"),
                ""
            ),
            AuthState::Ok
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
