//! Node self-registration against the central registry (issue #944).
//!
//! A worker node announces itself to the central node's `/api/registry/nodes`
//! so it becomes claim-eligible under the central claims API (#882). The
//! central refuses leases to nodes that never registered (`authorizeClaimRequest`
//! checks the registry), so without this a fresh worker configured with
//! `registry_central_url` can dispatch but never acquire a lease.
//!
//! Identity (`node_id`/`display_name`/`advertised_url`/`version`/
//! `schema_digest`) comes from the same `coordinator-identity.json`
//! `apps/server` reads/writes (`getCoordinatorIdentity()`), so there is one
//! identity-generation path rather than two that could drift -- the same
//! contract `resolve_node_id` in `crate::central_claims` already follows.
//!
//! Registration is idempotent from the caller's point of view: a 201 Created
//! and a 409 Conflict ("already registered") are both treated as success.
//! Any 4xx/5xx beyond 409 fails loudly -- the operator needs to know why the
//! central rejected the node rather than it silently not appearing in the
//! fleet.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use url::Url;

/// The identity file `apps/server` reads/writes, in priority order.
pub fn resolve_identity_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_COORDINATOR_IDENTITY_PATH") {
        return PathBuf::from(path);
    }
    let config_dir = crate::config::default_config_dir().join("coordinator-identity.json");
    if config_dir.exists() {
        return config_dir;
    }
    PathBuf::from("config/coordinator-identity.json")
}

#[derive(Debug, Deserialize)]
struct NodeIdentity {
    node_id: String,
    display_name: String,
    advertised_url: String,
    version: String,
    schema_digest: String,
}

fn read_identity() -> Result<NodeIdentity> {
    let path = resolve_identity_path();
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading node identity from {} (set GAH_COORDINATOR_IDENTITY_PATH if it lives elsewhere)",
            path.display()
        )
    })?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing node identity JSON from {}", path.display()))
}

pub struct RegisterOptions {
    pub central_url: String,
    /// Override for advertised_url (e.g. the node's tailnet address when the
    /// identity file predates the node joining the tailnet).
    pub advertised_url: Option<String>,
    pub transport_mode: String,
    pub secret_ref: String,
    /// Profiles this node declares it will dispatch. Defaults to every
    /// configured profile when None -- the central refuses a lease for any
    /// profile the node never declared (#881/#882).
    pub profiles: Option<Vec<String>>,
    pub token: Option<String>,
    pub config_path: Option<String>,
}

/// Transport abstraction so tests can substitute the curl subprocess with a
/// canned (status, body) response instead of standing up a real HTTP server.
pub trait RegisterTransport {
    /// Returns (http_status, response_body) on any completed HTTP exchange,
    /// `Err` only for transport-level failure (can't connect, timeout, DNS).
    /// A 4xx/5xx status is a normal `Ok` result the caller interprets.
    fn post(&self, url: &str, body: &str, token: Option<&str>) -> Result<(u16, Vec<u8>)>;
}

pub struct CurlRegisterTransport;

impl RegisterTransport for CurlRegisterTransport {
    fn post(&self, url: &str, body: &str, token: Option<&str>) -> Result<(u16, Vec<u8>)> {
        // Config-from-stdin (`-K -`) keeps the body and Bearer token out of
        // argv/ps, matching src/central_claims.rs and src/fleet_preflight.rs.
        let status_marker = "__GAH_REGISTER_STATUS__:";
        let mut cmd = Command::new("curl");
        cmd.args([
            "-sS",
            "--max-time",
            "15",
            "-K",
            "-",
            "-w",
            &format!("\n{status_marker}%{{http_code}}\n"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        crate::runner::process::arm_child_pdeathsig(&mut cmd);
        let mut child = cmd.spawn().context("spawning curl for node registration")?;
        if let Some(mut stdin) = child.stdin.take() {
            let escaped_url = url.replace('\\', "\\\\").replace('"', "\\\"");
            let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
            let mut config = format!(
                "silent\nurl = \"{escaped_url}\"\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\ndata = \"{escaped_body}\"\n"
            );
            if let Some(ca) = std::env::var("GAH_COORDINATOR_CA_CERT")
                .ok()
                .filter(|s| !s.is_empty())
            {
                let escaped_ca = ca.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!("cacert = \"{escaped_ca}\"\n"));
            } else if std::env::var("GAH_COORDINATOR_INSECURE_TLS")
                .ok()
                .as_deref()
                == Some("1")
            {
                config.push_str("insecure\n");
            }
            if let Some(t) = token {
                let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!("header = \"Authorization: Bearer {escaped}\"\n"));
            }
            stdin.write_all(config.as_bytes())?;
        }
        let output = child.wait_with_output().context("waiting for curl")?;
        if !output.status.success() {
            bail!(
                "node registration request failed (curl exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let marker_idx = stdout.rfind(status_marker).ok_or_else(|| {
            anyhow::anyhow!("node registration output missing status marker (malformed response?)")
        })?;
        let status: u16 = stdout[marker_idx + status_marker.len()..]
            .trim()
            .parse()
            .context("parsing HTTP status from node registration output")?;
        let response_body = stdout[..marker_idx].trim_end().as_bytes().to_vec();
        Ok((status, response_body))
    }
}

/// POSTs this node's identity to the central's `/api/registry/nodes`.
/// `Ok(())` means the node is registered (or already was). `Err` means the
/// central rejected it or the request failed -- the caller should surface
/// that loudly.
pub fn register(opts: &RegisterOptions) -> Result<()> {
    register_with(&CurlRegisterTransport, opts)
}

fn register_with(transport: &dyn RegisterTransport, opts: &RegisterOptions) -> Result<()> {
    let identity = read_identity()?;

    let profiles = match &opts.profiles {
        Some(p) => p.clone(),
        None => {
            let cfg = crate::config::load(opts.config_path.as_deref())?;
            let mut names: Vec<String> = cfg.profiles.keys().cloned().collect();
            names.sort_unstable();
            names
        }
    };

    let advertised_url = opts
        .advertised_url
        .clone()
        .unwrap_or(identity.advertised_url);
    let payload = serde_json::json!({
        "node_id": identity.node_id,
        "display_name": identity.display_name,
        "advertised_url": advertised_url,
        "version": identity.version,
        "schema_digest": identity.schema_digest,
        "transport_mode": opts.transport_mode,
        "secret_ref": opts.secret_ref,
        "profiles": profiles,
    });

    let mut url = Url::parse(&opts.central_url).context("parsing registry_central_url")?;
    url.set_path(&format!(
        "{}/api/registry/nodes",
        url.path().trim_end_matches('/')
    ));

    let (status, response_body) =
        transport.post(url.as_str(), &payload.to_string(), opts.token.as_deref())?;
    let body = String::from_utf8_lossy(&response_body);

    match status {
        // The server returns 201 on create and 409 on duplicate-ID. The 409
        // case means "already registered" -- idempotent success, not an error.
        201 => Ok(()),
        409 => {
            eprintln!(
                "gah node register: node already registered against {} (idempotent success)",
                opts.central_url
            );
            Ok(())
        }
        other => bail!("central rejected node registration (HTTP {other}): {body}"),
    }
}

/// Best-effort self-registration at `gah loop` startup (issue #944). Unlike
/// `register()`, a failure never aborts startup -- the loop can still do
/// useful single-node work, and the fleet preflight above already refused
/// (in `refuse` mode) if another node holds the profile. The operator gets a
/// loud warning so they know the node won't be claim-eligible until it's
/// registered or the central is reachable.
pub fn register_advisory(
    central_url: &str,
    transport_mode: &str,
    secret_ref: &str,
    token: Option<&str>,
) {
    match register(&RegisterOptions {
        central_url: central_url.to_string(),
        advertised_url: None,
        transport_mode: transport_mode.to_string(),
        secret_ref: secret_ref.to_string(),
        profiles: None,
        token: token.map(String::from),
        config_path: None,
    }) {
        Ok(()) => {}
        Err(e) => eprintln!(
            "gah loop: node self-registration against {central_url} failed ({e}). \
             The node may not be claim-eligible under central claims arbitration until this succeeds \
             -- re-run 'gah node register' once the central is reachable."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeTransport {
        status: RefCell<Vec<u16>>,
        body: String,
        last_url: RefCell<Option<String>>,
        last_body: RefCell<Option<String>>,
        calls: AtomicUsize,
    }

    impl FakeTransport {
        fn with_statuses(statuses: Vec<u16>) -> Self {
            Self {
                status: RefCell::new(statuses),
                body: r#"{"success":true}"#.to_string(),
                last_url: RefCell::new(None),
                last_body: RefCell::new(None),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl RegisterTransport for FakeTransport {
        fn post(&self, url: &str, body: &str, _token: Option<&str>) -> Result<(u16, Vec<u8>)> {
            self.last_url.replace(Some(url.to_string()));
            self.last_body.replace(Some(body.to_string()));
            let mut statuses = self.status.borrow_mut();
            let status = if statuses.is_empty() {
                201
            } else {
                statuses.remove(0)
            };
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((status, self.body.clone().into_bytes()))
        }
    }

    fn identity_path() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gah-node-register-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("coordinator-identity.json");
        std::fs::write(
            &path,
            r#"{"node_id":"node-abc","display_name":"Test Worker","advertised_url":"http://10.0.0.5:3773","version":"0.1.0","schema_digest":"abc"}"#,
        )
        .unwrap();
        path
    }

    fn options_with(
        advertised_url: Option<String>,
        profiles: Option<Vec<String>>,
    ) -> RegisterOptions {
        RegisterOptions {
            central_url: "http://central:3773".to_string(),
            advertised_url,
            transport_mode: "trusted_lan".to_string(),
            secret_ref: "env:COORDINATOR_TOKEN".to_string(),
            profiles,
            token: Some("tok".to_string()),
            config_path: None,
        }
    }

    #[test]
    fn register_builds_payload_and_hits_registry_path() {
        let path = identity_path();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let transport = FakeTransport::with_statuses(vec![201]);
        let opts = options_with(
            Some("http://10.0.0.5:3773".to_string()),
            Some(vec!["gah".to_string()]),
        );
        register_with(&transport, &opts).unwrap();

        let url = transport.last_url.borrow().clone().unwrap();
        assert!(url.ends_with("/api/registry/nodes"), "url={url}");
        let body = transport.last_body.borrow().clone().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["node_id"], "node-abc");
        assert_eq!(parsed["display_name"], "Test Worker");
        assert_eq!(parsed["advertised_url"], "http://10.0.0.5:3773");
        assert_eq!(parsed["transport_mode"], "trusted_lan");
        assert_eq!(parsed["secret_ref"], "env:COORDINATOR_TOKEN");
        assert_eq!(parsed["profiles"][0], "gah");
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
    }

    #[test]
    fn register_advertised_url_override_wins_over_identity() {
        let path = identity_path();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let transport = FakeTransport::with_statuses(vec![201]);
        let opts = options_with(
            Some("http://100.64.0.9:3773".to_string()),
            Some(vec!["gah".to_string()]),
        );
        register_with(&transport, &opts).unwrap();
        let body = transport.last_body.borrow().clone().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["advertised_url"], "http://100.64.0.9:3773");
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
    }

    #[test]
    fn register_201_is_success() {
        let path = identity_path();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let transport = FakeTransport::with_statuses(vec![201]);
        register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap();
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
    }

    #[test]
    fn register_409_is_idempotent_success() {
        let path = identity_path();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let transport = FakeTransport::with_statuses(vec![409]);
        register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap();
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
    }

    #[test]
    fn register_400_fails_loudly() {
        let path = identity_path();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let transport = FakeTransport::with_statuses(vec![400]);
        let err = register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap_err();
        assert!(err.to_string().contains("HTTP 400"), "{err}");
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
    }
}
