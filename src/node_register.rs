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
//! Registration is idempotent from the caller's point of view: the central
//! returns 201 on create and 200 after reconciling an existing registration.
//! Any 4xx/5xx fails loudly -- the operator needs to know why the
//! central rejected the node rather than it silently not appearing in the
//! fleet.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

/// The identity file `apps/server` reads/writes, in priority order.
pub fn resolve_identity_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_COORDINATOR_IDENTITY_PATH") {
        return PathBuf::from(path);
    }
    PathBuf::from("config/coordinator-identity.json")
}

#[derive(Deserialize)]
struct CoordinatorProtocol {
    version: String,
    schema_seed: String,
}

fn coordinator_protocol() -> CoordinatorProtocol {
    serde_json::from_str(include_str!(
        "../packages/contracts/src/coordinator-protocol.json"
    ))
    .expect("coordinator protocol manifest must be valid JSON")
}

fn default_version() -> String {
    coordinator_protocol().version
}

fn default_schema_digest() -> String {
    format!("{:x}", Sha256::digest(coordinator_protocol().schema_seed))
}

#[derive(Debug, Deserialize, Serialize)]
struct NodeIdentity {
    node_id: String,
    display_name: String,
    advertised_url: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default = "default_schema_digest")]
    schema_digest: String,
}

fn read_identity(path: &Path) -> Result<NodeIdentity> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading node identity from {} (set GAH_COORDINATOR_IDENTITY_PATH if it lives elsewhere)",
            path.display()
        )
    })?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing node identity JSON from {}", path.display()))
}

fn read_or_create_identity(path: &Path, advertised_url: Option<&str>) -> Result<NodeIdentity> {
    if path.exists() {
        return read_identity(path);
    }
    let advertised_url = advertised_url.ok_or_else(|| {
        anyhow::anyhow!(
            "node identity does not exist at {}; pass --advertised-url or set GAH_NODE_ADVERTISED_URL so it can be created",
            path.display()
        )
    })?;
    let display_name = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let identity = NodeIdentity {
        node_id: uuid::Uuid::new_v4().to_string(),
        display_name: if display_name.is_empty() {
            "GAH Worker".to_string()
        } else {
            display_name
        },
        advertised_url: advertised_url.to_string(),
        version: default_version(),
        schema_digest: default_schema_digest(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating identity directory {}", parent.display()))?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            serde_json::to_writer_pretty(&mut file, &identity)?;
            file.write_all(b"\n")?;
            Ok(identity)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_identity(path),
        Err(error) => {
            Err(error).with_context(|| format!("creating node identity at {}", path.display()))
        }
    }
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
    pub identity_path: Option<PathBuf>,
}

/// Transport abstraction so tests can substitute the curl subprocess with a
/// canned (status, body) response instead of standing up a real HTTP server.
trait RegisterTransport {
    /// Returns (http_status, response_body) on any completed HTTP exchange,
    /// `Err` only for transport-level failure (can't connect, timeout, DNS).
    /// A 4xx/5xx status is a normal `Ok` result the caller interprets.
    fn post(&self, url: &str, body: &str, token: Option<&str>) -> Result<(u16, Vec<u8>)>;
}

struct CurlRegisterTransport;

impl RegisterTransport for CurlRegisterTransport {
    fn post(&self, url: &str, body: &str, token: Option<&str>) -> Result<(u16, Vec<u8>)> {
        let response = crate::curl_http::request("POST", url, Some(body), token, 15)?;
        Ok((response.status, response.body))
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
    let identity_path = opts
        .identity_path
        .clone()
        .unwrap_or_else(resolve_identity_path);
    let identity = read_or_create_identity(&identity_path, opts.advertised_url.as_deref())?;

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
        // Existing registrations are reconciled on every loop start so profile
        // and endpoint changes cannot leave a node silently claim-ineligible.
        200 | 201 => Ok(()),
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
    config_path: Option<&str>,
) {
    match register(&RegisterOptions {
        central_url: central_url.to_string(),
        advertised_url: std::env::var("GAH_NODE_ADVERTISED_URL").ok(),
        transport_mode: transport_mode.to_string(),
        secret_ref: secret_ref.to_string(),
        profiles: None,
        token: token.map(String::from),
        config_path: config_path.map(String::from),
        identity_path: None,
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
            identity_path: Some(identity_path()),
        }
    }

    #[test]
    fn register_builds_payload_and_hits_registry_path() {
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
    }

    #[test]
    fn register_advertised_url_override_wins_over_identity() {
        let transport = FakeTransport::with_statuses(vec![201]);
        let opts = options_with(
            Some("http://100.64.0.9:3773".to_string()),
            Some(vec!["gah".to_string()]),
        );
        register_with(&transport, &opts).unwrap();
        let body = transport.last_body.borrow().clone().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["advertised_url"], "http://100.64.0.9:3773");
    }

    #[test]
    fn register_201_is_success() {
        let transport = FakeTransport::with_statuses(vec![201]);
        register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap();
    }

    #[test]
    fn register_200_update_is_success() {
        let transport = FakeTransport::with_statuses(vec![200]);
        register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap();
    }

    #[test]
    fn register_400_fails_loudly() {
        let transport = FakeTransport::with_statuses(vec![400]);
        let err = register_with(
            &transport,
            &options_with(None, Some(vec!["gah".to_string()])),
        )
        .unwrap_err();
        assert!(err.to_string().contains("HTTP 400"), "{err}");
    }

    #[test]
    fn identity_written_by_server_gets_derived_contract_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("coordinator-identity.json");
        std::fs::write(
            &path,
            r#"{"node_id":"node-abc","display_name":"Worker","advertised_url":"http://10.0.0.5:3773"}"#,
        )
        .unwrap();
        let identity = read_identity(&path).unwrap();
        assert_eq!(identity.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(identity.schema_digest, default_schema_digest());
    }

    #[test]
    fn missing_identity_is_created_when_advertised_url_is_configured() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("coordinator-identity.json");
        let identity = read_or_create_identity(&path, Some("http://10.0.0.5:3773")).unwrap();
        assert_eq!(identity.advertised_url, "http://10.0.0.5:3773");
        assert_eq!(read_identity(&path).unwrap().node_id, identity.node_id);
    }
}
