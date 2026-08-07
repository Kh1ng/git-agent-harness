//! Central claim arbitration client (issue #882): the Rust side of the
//! cross-node exclusivity gate whose server half lives in
//! `apps/server/src/claimsService.ts`. Replaces the local claim-mode
//! ledger entry (`src/dispatch/claims.rs`'s `check_duplicate_work`) as the
//! authoritative "has someone else already picked up this work_id" check
//! -- but only when `Defaults::registry_central_url` is configured; unset
//! means no behavior change from before this module existed.
//!
//! Design decisions locked in for this ticket (recorded here since they
//! were real architecture choices, not defaults reached by elimination):
//! - **Fail closed.** If the central node can't be reached, or another
//!   node already holds the claim, dispatch does not start. A central
//!   outage stopping the whole fleet is the accepted tradeoff for never
//!   silently double-dispatching a ticket.
//! - **Plain HTTPS polling, not a persistent connection.** The end-state
//!   architecture is nodes dialing in to the central node (never the
//!   reverse -- avoids exposing a surface on every worker), which a
//!   worker-initiated HTTP call already satisfies. A future upgrade to a
//!   long-lived connection (WebSocket, with an HTTP fallback) is a
//!   transport change behind this same acquire/renew/release shape, not a
//!   redesign.
//! - **Renewal, not a flat TTL.** A dispatch that's still genuinely
//!   running periodically extends its own lease (every `lease/3`) instead
//!   of relying on one long-enough timeout. If renewal has been failing
//!   long enough that the lease would have lapsed, the guard stops trying
//!   to pretend it still holds the claim (logs loudly) rather than
//!   silently continuing past an exclusivity guarantee that's no longer
//!   backed by anything -- but it does not kill the in-progress backend
//!   process; that's a larger control-flow change left for later if
//!   experience shows it's needed.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const DEFAULT_LEASE_SECONDS: u64 = 15 * 60;
const STATUS_MARKER: &str = "__GAH_CLAIMS_STATUS__:";

/// Resolves this node's identity for claims calls from the same
/// `coordinator-identity.json` `apps/server` reads/writes
/// (`getCoordinatorIdentity()`), rather than generating a second one that
/// could drift. `GAH_COORDINATOR_IDENTITY_PATH`, if set, must match
/// whatever `apps/server` was started with; the default assumes `gah` and
/// `apps/server` run from the same working directory (true for the
/// documented systemd setup in docs/OPERATIONS.md). Missing/unreadable
/// fails loudly rather than guessing a node_id -- consistent with this
/// module's fail-closed posture.
pub fn resolve_node_id() -> Result<String> {
    let path = std::env::var("GAH_COORDINATOR_IDENTITY_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config/coordinator-identity.json"));
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading coordinator identity from {} (set GAH_COORDINATOR_IDENTITY_PATH if apps/server uses a different path)",
            path.display()
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context("parsing coordinator identity JSON")?;
    value
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("{} has no node_id field", path.display()))
}

pub trait ClaimsTransport {
    /// Returns (http_status, response_body) on any completed HTTP
    /// exchange, `Err` only for transport-level failure (can't connect,
    /// timeout, DNS) -- a 409 Conflict is a normal `Ok` result the caller
    /// interprets, not a transport error.
    fn post(
        &self,
        url: &str,
        body: &str,
        token: Option<&str>,
        timeout_secs: u32,
    ) -> Result<(u16, Vec<u8>)>;
}

pub struct CurlClaimsTransport;

impl ClaimsTransport for CurlClaimsTransport {
    fn post(
        &self,
        url: &str,
        body: &str,
        token: Option<&str>,
        timeout_secs: u32,
    ) -> Result<(u16, Vec<u8>)> {
        // Config-from-stdin (`-K -`) keeps the body and bearer token out of
        // argv/ps, matching src/usage/vibe_admin.rs's existing pattern.
        // `-w` appends a greppable status marker after the response body
        // instead of `--fail`, which would discard the JSON error body
        // (e.g. a 409's `held_by`) that callers need to read.
        let mut cmd = Command::new("curl");
        cmd.args([
            "-sS",
            "--max-time",
            &timeout_secs.to_string(),
            "-K",
            "-",
            "-w",
            &format!("\n{STATUS_MARKER}%{{http_code}}\n"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        crate::runner::process::arm_child_pdeathsig(&mut cmd);
        let mut child = cmd
            .spawn()
            .context("spawning curl for central claims call")?;
        if let Some(mut stdin) = child.stdin.take() {
            let escaped_url = url.replace('\\', "\\\\").replace('"', "\\\"");
            let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
            let mut config = format!(
                "silent\nurl = \"{escaped_url}\"\nheader = \"Content-Type: application/json\"\ndata = \"{escaped_body}\"\n"
            );
            if let Some(t) = token {
                let escaped_token = t.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!(
                    "header = \"Authorization: Bearer {escaped_token}\"\n"
                ));
            }
            stdin.write_all(config.as_bytes())?;
        }
        let output = child.wait_with_output().context("waiting for curl")?;
        if !output.status.success() {
            anyhow::bail!(
                "central claims request failed (curl exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let marker_idx = stdout.rfind(STATUS_MARKER).ok_or_else(|| {
            anyhow::anyhow!("curl output missing status marker (malformed response?)")
        })?;
        let status: u16 = stdout[marker_idx + STATUS_MARKER.len()..]
            .trim()
            .parse()
            .context("parsing HTTP status from curl output")?;
        let response_body = stdout[..marker_idx].trim_end().as_bytes().to_vec();
        Ok((status, response_body))
    }
}

#[derive(Debug, Deserialize)]
struct LeaseResponse {
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct ConflictResponse {
    message: String,
    held_by: HeldBy,
}

#[derive(Debug, Deserialize)]
struct HeldBy {
    node_id: String,
}

fn claims_url(central_url: &str, path: &str) -> Result<String> {
    let mut url = Url::parse(central_url).context("parsing registry_central_url")?;
    url.set_path(&format!(
        "{}/api/claims/{path}",
        url.path().trim_end_matches('/')
    ));
    Ok(url.to_string())
}

fn request_body(node_id: &str, profile: &str, work_id: &str, lease_seconds: Option<u64>) -> String {
    let mut obj = serde_json::json!({
        "node_id": node_id,
        "profile": profile,
        "work_id": work_id,
    });
    if let Some(lease) = lease_seconds {
        obj["lease_seconds"] = serde_json::json!(lease);
    }
    obj.to_string()
}

/// `Err` on any transport failure, a non-{200,409} status, or a 409
/// (someone else holds the claim) -- callers treat all three as "do not
/// proceed," matching this module's fail-closed policy. The 409 case's
/// message names the holder.
#[allow(clippy::too_many_arguments)]
fn acquire_or_renew(
    transport: &dyn ClaimsTransport,
    path: &str,
    central_url: &str,
    node_id: &str,
    profile: &str,
    work_id: &str,
    token: Option<&str>,
    lease_seconds: u64,
) -> Result<String> {
    let url = claims_url(central_url, path)?;
    let body = request_body(node_id, profile, work_id, Some(lease_seconds));
    let (status, response_body) = transport.post(&url, &body, token, 15)?;
    match status {
        200 => {
            let lease: LeaseResponse =
                serde_json::from_slice(&response_body).context("parsing claim lease response")?;
            Ok(lease.expires_at)
        }
        409 => {
            let conflict: ConflictResponse =
                serde_json::from_slice(&response_body).unwrap_or(ConflictResponse {
                    message: "conflict".into(),
                    held_by: HeldBy {
                        node_id: "unknown".into(),
                    },
                });
            anyhow::bail!(
                "refusing to start: central claims API says work_id '{work_id}' on profile '{profile}' is held by node '{}': {}",
                conflict.held_by.node_id,
                conflict.message
            );
        }
        other => anyhow::bail!(
            "central claims API returned unexpected status {other}: {}",
            String::from_utf8_lossy(&response_body)
        ),
    }
}

/// Acquires a claim and starts a background renewal thread for it. The
/// returned guard releases the claim (best-effort) and stops the renewal
/// thread when dropped -- hold it for the duration of the dispatch that's
/// meant to be exclusive.
pub fn acquire(
    central_url: &str,
    node_id: &str,
    profile: &str,
    work_id: &str,
    token: Option<&str>,
) -> Result<ClaimGuard> {
    acquire_or_renew(
        &CurlClaimsTransport,
        "acquire",
        central_url,
        node_id,
        profile,
        work_id,
        token,
        DEFAULT_LEASE_SECONDS,
    )?;

    let stop = Arc::new(AtomicBool::new(false));
    let renewal_thread = {
        let stop = Arc::clone(&stop);
        let central_url = central_url.to_string();
        let node_id = node_id.to_string();
        let profile = profile.to_string();
        let work_id = work_id.to_string();
        // Owned copy for the 'static thread closure -- `token: Option<&str>`
        // can't outlive this function's stack frame.
        let token = token.map(String::from);
        std::thread::spawn(move || {
            let renew_every = Duration::from_secs(DEFAULT_LEASE_SECONDS / 3);
            let mut consecutive_failures = 0u32;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(renew_every);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match acquire_or_renew(
                    &CurlClaimsTransport,
                    "renew",
                    &central_url,
                    &node_id,
                    &profile,
                    &work_id,
                    token.as_deref(),
                    DEFAULT_LEASE_SECONDS,
                ) {
                    Ok(_) => consecutive_failures = 0,
                    Err(e) => {
                        consecutive_failures += 1;
                        eprintln!("gah: central claim renewal failed for {profile}/{work_id} ({consecutive_failures} consecutive failure(s)): {e:#}");
                        // 3 missed renewals at lease/3 spacing == roughly one
                        // full lease window with no successful renewal --
                        // the claim has almost certainly lapsed centrally by
                        // now. Keep trying (the central node or network may
                        // recover), but stop pretending exclusivity is still
                        // guaranteed; see module docs for why this doesn't
                        // kill the in-progress backend outright.
                        if consecutive_failures == 3 {
                            eprintln!("gah: WARNING -- central claim for {profile}/{work_id} has likely lapsed; another node may now be able to claim it. Dispatch is continuing (not killed), but exclusivity is no longer guaranteed.");
                        }
                    }
                }
            }
        })
    };

    Ok(ClaimGuard {
        central_url: central_url.to_string(),
        node_id: node_id.to_string(),
        profile: profile.to_string(),
        work_id: work_id.to_string(),
        token: token.map(String::from),
        stop,
        renewal_thread: Some(renewal_thread),
    })
}

pub struct ClaimGuard {
    central_url: String,
    node_id: String,
    profile: String,
    work_id: String,
    token: Option<String>,
    stop: Arc<AtomicBool>,
    renewal_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Best-effort release -- a failure here just means the lease
        // expires naturally on the server side instead of being cleared
        // early. Never panics or blocks dispatch's own exit on this.
        let url = match claims_url(&self.central_url, "release") {
            Ok(u) => u,
            Err(_) => return,
        };
        let body = request_body(&self.node_id, &self.profile, &self.work_id, None);
        let _ = CurlClaimsTransport.post(&url, &body, self.token.as_deref(), 5);
        // Don't join the renewal thread here: it's sleeping in
        // renew_every-sized increments and may not wake for minutes. The
        // stop flag it already saw means it exits on its next wake; the
        // thread is daemon-like (process exit reaps it) rather than
        // something dispatch must wait on.
        self.renewal_thread.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Mutex;

    // Both resolve_node_id tests mutate the process-global
    // GAH_COORDINATOR_IDENTITY_PATH env var; cargo test runs tests in
    // parallel threads within one process, so without this they can
    // interleave and read each other's value (the same class of flake
    // fixed for TICKET-180's canonical-config tests).
    static ENV_VAR_TEST_LOCK: Mutex<()> = Mutex::new(());

    type FakeResponse = std::result::Result<(u16, Vec<u8>), String>;

    struct FakeTransport {
        responses: RefCell<std::collections::VecDeque<FakeResponse>>,
        calls: RefCell<Vec<(String, String)>>,
    }

    impl FakeTransport {
        fn queue(responses: Vec<(u16, &str)>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(s, b)| Ok((s, b.as_bytes().to_vec())))
                        .collect(),
                ),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ClaimsTransport for FakeTransport {
        fn post(
            &self,
            url: &str,
            body: &str,
            _token: Option<&str>,
            _timeout_secs: u32,
        ) -> Result<(u16, Vec<u8>)> {
            self.calls
                .borrow_mut()
                .push((url.to_string(), body.to_string()));
            match self.responses.borrow_mut().pop_front() {
                Some(Ok(r)) => Ok(r),
                Some(Err(e)) => anyhow::bail!("{e}"),
                None => anyhow::bail!("FakeTransport: no more queued responses"),
            }
        }
    }

    #[test]
    fn acquire_succeeds_on_200_and_returns_expiry() {
        let t = FakeTransport::queue(vec![(200, r#"{"expires_at":"2026-01-01T00:00:00Z"}"#)]);
        let expires = acquire_or_renew(
            &t,
            "acquire",
            "http://central:3773",
            "node-a",
            "gah",
            "ticket-1",
            None,
            900,
        )
        .unwrap();
        assert_eq!(expires, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn acquire_fails_closed_on_409_naming_the_holder() {
        let t = FakeTransport::queue(vec![(
            409,
            r#"{"error":"Conflict","message":"already claimed","held_by":{"node_id":"node-b","profile":"gah","work_id":"ticket-1","claimed_at":"","renewed_at":"","expires_at":""}}"#,
        )]);
        let err = acquire_or_renew(
            &t,
            "acquire",
            "http://central:3773",
            "node-a",
            "gah",
            "ticket-1",
            None,
            900,
        )
        .expect_err("must fail closed on conflict");
        assert!(err.to_string().contains("node-b"));
    }

    #[test]
    fn acquire_fails_closed_on_transport_error() {
        let t = FakeTransport {
            responses: RefCell::new(std::collections::VecDeque::from([Err(
                "connection refused".to_string(),
            )])),
            calls: RefCell::new(Vec::new()),
        };
        assert!(acquire_or_renew(
            &t,
            "acquire",
            "http://central:3773",
            "node-a",
            "gah",
            "ticket-1",
            None,
            900
        )
        .is_err());
    }

    #[test]
    fn acquire_fails_closed_on_unexpected_status() {
        let t = FakeTransport::queue(vec![(500, "internal error")]);
        assert!(acquire_or_renew(
            &t,
            "acquire",
            "http://central:3773",
            "node-a",
            "gah",
            "ticket-1",
            None,
            900
        )
        .is_err());
    }

    #[test]
    fn request_url_and_body_are_built_correctly() {
        let t = FakeTransport::queue(vec![(200, r#"{"expires_at":"x"}"#)]);
        acquire_or_renew(
            &t,
            "acquire",
            "http://central:3773/",
            "node-a",
            "gah",
            "ticket-1",
            None,
            900,
        )
        .unwrap();
        let calls = t.calls.borrow();
        assert_eq!(calls[0].0, "http://central:3773/api/claims/acquire");
        let body: serde_json::Value = serde_json::from_str(&calls[0].1).unwrap();
        assert_eq!(body["node_id"], "node-a");
        assert_eq!(body["profile"], "gah");
        assert_eq!(body["work_id"], "ticket-1");
        assert_eq!(body["lease_seconds"], 900);
    }

    #[test]
    fn resolve_node_id_reads_the_identity_file() {
        let _lock = ENV_VAR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("coordinator-identity.json");
        std::fs::write(&path, r#"{"node_id":"abc-123","display_name":"Test"}"#).unwrap();
        std::env::set_var("GAH_COORDINATOR_IDENTITY_PATH", &path);
        let result = resolve_node_id();
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
        assert_eq!(result.unwrap(), "abc-123");
    }

    #[test]
    fn resolve_node_id_fails_closed_when_the_file_is_missing() {
        let _lock = ENV_VAR_TEST_LOCK.lock().unwrap();
        std::env::set_var(
            "GAH_COORDINATOR_IDENTITY_PATH",
            "/nonexistent/path/coordinator-identity.json",
        );
        let result = resolve_node_id();
        std::env::remove_var("GAH_COORDINATOR_IDENTITY_PATH");
        assert!(result.is_err());
    }
}
