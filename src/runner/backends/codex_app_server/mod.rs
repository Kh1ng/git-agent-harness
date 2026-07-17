//! Managed Codex `app-server` adapter (issue #527, 1/6): a versioned
//! JSON-RPC transport over stdio or a Unix socket, with an
//! initialize/initialized handshake, stable-vs-experimental capability
//! negotiation, and typed retention of unknown server events.
//!
//! `codex exec --json` (see `runner::backends::codex`) remains the
//! tested, unchanged fallback -- nothing here replaces it. This module is
//! additive plumbing that later tickets in the series wire into real
//! dispatch (thread/turn calls); ticket 1 only needs the transport
//! contract and generated-schema bookkeeping to exist and be correct.

pub(crate) mod protocol;
pub(crate) mod transport;
pub(crate) mod version;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

// Re-exported for the ticket in this series that wires a real session
// into dispatch; nothing outside this module's own tests calls these yet.
#[allow(unused_imports)]
pub(crate) use protocol::{JsonRpcError, RequestId, ServerEvent};
#[allow(unused_imports)]
pub(crate) use transport::{StdioTransport, Transport, TransportEvent};
#[allow(unused_imports)]
pub(crate) use version::{
    check_version_drift, compute_schema_digest, detect_codex_version, load_schema_manifest,
    record_codex_app_server_info, CodexAppServerInfo, CodexSchemaManifest,
};

#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use transport::UnixSocketTransport;

use protocol::IncomingMessage;

const CLIENT_NAME: &str = "gah";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RECONNECT_ATTEMPTS: u32 = 3;

/// Where to reach the app-server. Both variants are client-only: GAH
/// either spawns the child itself (stdio) or dials a socket path that
/// already exists (Unix socket) -- it never creates a listener.
#[derive(Debug, Clone)]
pub(crate) enum AppServerTarget {
    Stdio {
        executable: PathBuf,
        args: Vec<String>,
    },
    #[cfg(unix)]
    UnixSocket { path: PathBuf },
}

/// Client-declared capability negotiation. Stable APIs are the default;
/// experimental methods require the caller to explicitly opt in, both by
/// setting `experimentalApi: true` during `initialize` and (client-side,
/// defense in depth alongside the server's own enforcement) by refusing
/// to send any method named in `experimental_methods` unless enabled.
#[derive(Debug, Clone, Default)]
pub(crate) struct CodexAppServerOptions {
    pub(crate) experimental_enabled: bool,
    pub(crate) experimental_methods: HashSet<String>,
}

impl CodexAppServerOptions {
    /// Builds options from a generated-package manifest
    /// (`scripts/generate-codex-schemas.js` output): the manifest's
    /// experimental-methods list becomes the client-side gate, and
    /// `experimental_enabled` stays an explicit, separate opt-in the
    /// manifest itself cannot flip on.
    #[allow(dead_code)]
    pub(crate) fn from_manifest(
        manifest: &CodexSchemaManifest,
        experimental_enabled: bool,
    ) -> Self {
        Self {
            experimental_enabled,
            experimental_methods: manifest.experimental_methods.iter().cloned().collect(),
        }
    }

    fn rejects(&self, method: &str) -> bool {
        !self.experimental_enabled && self.experimental_methods.contains(method)
    }
}

/// Outcome of a single request/response call, with overload errors
/// classified out from hard failures so callers can retry/backoff instead
/// of treating an overloaded upstream as a fatal protocol error.
#[derive(Debug)]
pub(crate) enum CallOutcome {
    Result(Value),
    Overloaded(JsonRpcError),
    Error(JsonRpcError),
}

/// A live app-server connection: handshake already completed, ready for
/// request/response calls. Notifications and server-requests that arrive
/// while waiting on an unrelated call are retained in `pending_events`
/// rather than dropped (typed-unknown-event retention).
pub(crate) struct CodexAppServerSession {
    target: AppServerTarget,
    worktree: PathBuf,
    env_vars: Vec<(String, String)>,
    stderr_log_path: PathBuf,
    options: CodexAppServerOptions,
    transport: Box<dyn Transport>,
    next_id: i64,
    handshake_result: Value,
    reconnect_attempts: u32,
    pending_events: Vec<ServerEvent>,
}

fn spawn_transport(
    target: &AppServerTarget,
    worktree: &Path,
    env_vars: &[(String, String)],
    stderr_log_path: &Path,
) -> Result<Box<dyn Transport>> {
    match target {
        AppServerTarget::Stdio { executable, args } => Ok(Box::new(StdioTransport::spawn(
            executable,
            args,
            worktree,
            env_vars,
            stderr_log_path,
        )?)),
        #[cfg(unix)]
        AppServerTarget::UnixSocket { path } => Ok(Box::new(UnixSocketTransport::connect(path)?)),
    }
}

impl CodexAppServerSession {
    pub(crate) fn connect(
        target: AppServerTarget,
        worktree: &Path,
        env_vars: &[(String, String)],
        session_dir: &Path,
        options: CodexAppServerOptions,
    ) -> Result<Self> {
        let stderr_log_path = session_dir.join("codex-app-server-stderr.log");
        let transport = spawn_transport(&target, worktree, env_vars, &stderr_log_path)?;
        let mut session = Self {
            target,
            worktree: worktree.to_path_buf(),
            env_vars: env_vars.to_vec(),
            stderr_log_path,
            options,
            transport,
            next_id: 1,
            handshake_result: Value::Null,
            reconnect_attempts: 0,
            pending_events: Vec::new(),
        };
        session.handshake()?;
        Ok(session)
    }

    fn allocate_id(&mut self) -> RequestId {
        let id = RequestId::Number(self.next_id);
        self.next_id += 1;
        id
    }

    fn handshake(&mut self) -> Result<()> {
        let id = self.allocate_id();
        let params = json!({
            "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
            "capabilities": { "experimentalApi": self.options.experimental_enabled },
        });
        self.transport.send_line(&json!({
            "jsonrpc": "2.0",
            "id": &id,
            "method": "initialize",
            "params": params,
        }))?;

        match self.wait_for(&id, DEFAULT_HANDSHAKE_TIMEOUT)? {
            Ok(result) => self.handshake_result = result,
            Err(error) => anyhow::bail!(
                "codex app-server rejected initialize (code {}): {}",
                error.code,
                error.message
            ),
        }

        self.transport.send_line(&json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {},
        }))
    }

    /// The raw `initialize` response (userAgent/codexHome/platform fields
    /// on the installed binary), kept opaque here -- ticket 1 only needs
    /// to prove the handshake completed, not interpret every field.
    pub(crate) fn handshake_result(&self) -> &Value {
        &self.handshake_result
    }

    /// Sends a request and waits for its matching response, classifying
    /// overload errors out from hard errors. Rejects experimental methods
    /// client-side (before writing to the wire) unless explicitly opted
    /// in, mirroring the server's own `experimentalApi` enforcement.
    pub(crate) fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<CallOutcome> {
        if self.options.rejects(method) {
            anyhow::bail!(
                "codex app-server method '{method}' is experimental; \
                 set experimental_enabled to opt in"
            );
        }
        let id = self.allocate_id();
        self.transport.send_line(&json!({
            "jsonrpc": "2.0",
            "id": &id,
            "method": method,
            "params": params,
        }))?;
        match self.wait_for(&id, timeout)? {
            Ok(result) => Ok(CallOutcome::Result(result)),
            Err(error) => {
                if protocol::is_overload_error(&error) {
                    Ok(CallOutcome::Overloaded(error))
                } else {
                    Ok(CallOutcome::Error(error))
                }
            }
        }
    }

    /// Blocks until the response for `expected_id` arrives, a fatal
    /// transport condition occurs, or `timeout` elapses. Out-of-band
    /// notifications/server-requests seen along the way are classified
    /// and stashed in `pending_events` (drained via [`Self::drain_events`])
    /// instead of being discarded.
    fn wait_for(
        &mut self,
        expected_id: &RequestId,
        timeout: Duration,
    ) -> Result<Result<Value, JsonRpcError>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!(
                    "timed out waiting for codex app-server response to id {expected_id}"
                );
            }
            match self.transport.recv_timeout(remaining) {
                TransportEvent::Closed(code) => {
                    anyhow::bail!(
                        "codex app-server closed the connection (exit {code:?}) \
                         before responding to id {expected_id}"
                    );
                }
                TransportEvent::Malformed { raw, reason } => {
                    anyhow::bail!("codex app-server sent an unparseable message ({reason}): {raw}");
                }
                TransportEvent::Message(IncomingMessage::Response { id, result })
                    if id == *expected_id =>
                {
                    return Ok(Ok(result));
                }
                TransportEvent::Message(IncomingMessage::ErrorResponse { id, error })
                    if id == *expected_id =>
                {
                    return Ok(Err(error));
                }
                TransportEvent::Message(IncomingMessage::Response { .. })
                | TransportEvent::Message(IncomingMessage::ErrorResponse { .. }) => {
                    // A response to some earlier/stale id GAH is no longer
                    // waiting on; nothing meaningful to retain.
                    continue;
                }
                TransportEvent::Message(IncomingMessage::Notification { method, params }) => {
                    self.pending_events
                        .push(protocol::classify_server_event(method, params));
                }
                TransportEvent::Message(IncomingMessage::ServerRequest {
                    method, params, ..
                }) => {
                    // No approval/tool-call handler exists yet in this
                    // ticket's scope -- retained as a typed event rather
                    // than silently dropped. A later ticket that starts
                    // real threads/turns must answer these instead.
                    self.pending_events
                        .push(protocol::classify_server_event(method, params));
                }
            }
        }
    }

    /// Drains server-originated notifications/requests observed while
    /// waiting on calls, so callers can inspect (and eventually act on)
    /// events this ticket's transport layer doesn't interpret itself.
    pub(crate) fn drain_events(&mut self) -> Vec<ServerEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Tears down the current transport and re-establishes it (new
    /// process or new socket connection, per `target`), then re-runs the
    /// handshake. Bounded by `MAX_RECONNECT_ATTEMPTS` so a persistently
    /// broken app-server fails visibly instead of retrying forever.
    pub(crate) fn reconnect(&mut self) -> Result<()> {
        if self.reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
            anyhow::bail!("codex app-server exceeded {MAX_RECONNECT_ATTEMPTS} reconnect attempts");
        }
        self.reconnect_attempts += 1;

        let new_transport = spawn_transport(
            &self.target,
            &self.worktree,
            &self.env_vars,
            &self.stderr_log_path,
        )
        .context("reconnecting to codex app-server")?;
        let old_transport = std::mem::replace(&mut self.transport, new_transport);
        old_transport.shutdown();

        // `next_id` is intentionally left running rather than reset to 1:
        // the fresh app-server process has no memory of previously issued
        // ids, but resetting here would let a new request reuse an id a
        // caller may still associate with a pre-reconnect response.
        self.handshake_result = Value::Null;
        self.handshake()
    }

    pub(crate) fn shutdown(self) {
        self.transport.shutdown();
    }

    #[cfg(test)]
    pub(crate) fn next_id_for_test(&self) -> i64 {
        self.next_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    /// A POSIX-sh fake `codex app-server`: reads newline-delimited
    /// JSON-RPC requests and replies deterministically by matching on
    /// substrings of the raw line (no JSON parser available in `sh`).
    /// This is the "fake app-server protocol tests" verification bucket.
    const FAKE_APP_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
      case "$line" in
        *'"experimentalApi":true'*)
          printf '{"id":%s,"result":{"userAgent":"fake/9.9.9","codexHome":"/fake","platformFamily":"unix","platformOs":"linux","experimentalApi":true}}\n' "$id"
          ;;
        *)
          printf '{"id":%s,"result":{"userAgent":"fake/9.9.9","codexHome":"/fake","platformFamily":"unix","platformOs":"linux"}}\n' "$id"
          ;;
      esac
      ;;
    *'"method":"initialized"'*)
      printf '{"method":"remoteControl/status/changed","params":{"status":"disabled"}}\n'
      ;;
    *'"method":"mock/overload"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
      printf '{"id":%s,"error":{"code":-32000,"message":"server overloaded","data":{"codexErrorInfo":"serverOverloaded"}}}\n' "$id"
      ;;
    *'"method":"mock/experimentalMethod"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
      printf '{"id":%s,"result":{"echoed":null}}\n' "$id"
      ;;
    *'"method":"weird/unknown-method"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
      printf '{"id":%s,"error":{"code":-32600,"message":"Invalid request: unknown variant weird/unknown-method"}}\n' "$id"
      ;;
  esac
done
"#;

    fn connect_to_fake(f: &Fixture, options: CodexAppServerOptions) -> CodexAppServerSession {
        make_fake_bin(&f.bin_dir, "fake-app-server", FAKE_APP_SERVER);
        CodexAppServerSession::connect(
            AppServerTarget::Stdio {
                executable: f.bin_dir.join("fake-app-server"),
                args: vec![],
            },
            &f.worktree,
            &[],
            &f.session_dir,
            options,
        )
        .unwrap()
    }

    #[test]
    fn handshake_completes_against_fake_app_server() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let session = connect_to_fake(&f, CodexAppServerOptions::default());

        assert_eq!(session.handshake_result()["userAgent"], "fake/9.9.9");
        session.shutdown();
    }

    #[test]
    fn stable_is_the_default_capability_sent_during_initialize() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let session = connect_to_fake(&f, CodexAppServerOptions::default());

        assert!(session.handshake_result().get("experimentalApi").is_none());
        session.shutdown();
    }

    #[test]
    fn experimental_opt_in_is_reflected_in_the_handshake() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let session = connect_to_fake(
            &f,
            CodexAppServerOptions {
                experimental_enabled: true,
                experimental_methods: HashSet::new(),
            },
        );

        assert_eq!(session.handshake_result()["experimentalApi"], true);
        session.shutdown();
    }

    #[test]
    fn unknown_method_error_round_trips_as_a_typed_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        let outcome = session
            .call("weird/unknown-method", json!({}), Duration::from_secs(5))
            .unwrap();
        match outcome {
            CallOutcome::Error(error) => {
                assert_eq!(error.code, -32600);
                assert!(error.message.contains("unknown variant"));
            }
            other => panic!("expected Error outcome, got {other:?}"),
        }
        session.shutdown();
    }

    #[test]
    fn overload_error_is_classified_separately_from_hard_errors() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        let outcome = session
            .call("mock/overload", json!({}), Duration::from_secs(5))
            .unwrap();
        match outcome {
            CallOutcome::Overloaded(error) => assert_eq!(error.code, -32000),
            other => panic!("expected Overloaded outcome, got {other:?}"),
        }
        session.shutdown();
    }

    #[test]
    fn options_from_manifest_gate_exactly_the_manifests_experimental_methods() {
        let manifest = CodexSchemaManifest {
            codex_binary_version: "0.144.5".to_string(),
            schema_digest: "sha256:abc".to_string(),
            experimental_methods: vec!["process/spawn".to_string()],
        };

        let stable = CodexAppServerOptions::from_manifest(&manifest, false);
        assert!(stable.rejects("process/spawn"));
        assert!(!stable.rejects("thread/start"));

        let opted_in = CodexAppServerOptions::from_manifest(&manifest, true);
        assert!(!opted_in.rejects("process/spawn"));
    }

    #[test]
    fn experimental_method_is_rejected_client_side_without_opt_in() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut methods = HashSet::new();
        methods.insert("mock/experimentalMethod".to_string());
        let mut session = connect_to_fake(
            &f,
            CodexAppServerOptions {
                experimental_enabled: false,
                experimental_methods: methods,
            },
        );

        let err = session
            .call("mock/experimentalMethod", json!({}), Duration::from_secs(5))
            .unwrap_err();
        assert!(err.to_string().contains("experimental"));
        session.shutdown();
    }

    #[test]
    fn experimental_method_succeeds_once_opted_in() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut methods = HashSet::new();
        methods.insert("mock/experimentalMethod".to_string());
        let mut session = connect_to_fake(
            &f,
            CodexAppServerOptions {
                experimental_enabled: true,
                experimental_methods: methods,
            },
        );

        let outcome = session
            .call("mock/experimentalMethod", json!({}), Duration::from_secs(5))
            .unwrap();
        match outcome {
            CallOutcome::Result(value) => assert_eq!(value, json!({"echoed": null})),
            other => panic!("expected Result outcome, got {other:?}"),
        }
        session.shutdown();
    }

    #[test]
    fn unsolicited_notification_is_retained_via_drain_events() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        // `initialized` triggers the fake server's unsolicited
        // `remoteControl/status/changed` notification; force it to be
        // observed by making an unrelated call and waiting on its
        // response.
        let _ = session
            .call("mock/overload", json!({}), Duration::from_secs(5))
            .unwrap();

        let events = session.drain_events();
        assert!(events
            .iter()
            .any(|event| event.method == "remoteControl/status/changed"));
        // No known variant is modeled yet in this ticket -- confirms the
        // event was retained typed-but-unknown rather than dropped.
        assert!(events.iter().all(|event| !event.known));
        session.shutdown();
    }

    #[test]
    fn reconnect_re_establishes_a_working_session() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        session.reconnect().unwrap();

        let outcome = session
            .call("mock/overload", json!({}), Duration::from_secs(5))
            .unwrap();
        assert!(matches!(outcome, CallOutcome::Overloaded(_)));
        session.shutdown();
    }

    #[test]
    fn reconnect_does_not_reuse_request_ids_across_multiple_reconnects() {
        // Regression test: `reconnect()` used to reset `next_id` to 1 on
        // every call, so id 1 would be reissued after the 1st reconnect
        // *and* again after the 2nd. Ids must stay unique for the life of
        // the session so a caller that retained an id across a reconnect
        // never sees it silently aliased to an unrelated later call.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        let id_before = session.next_id_for_test();
        session.reconnect().unwrap();
        let id_after_first_reconnect = session.next_id_for_test();
        session.reconnect().unwrap();
        let id_after_second_reconnect = session.next_id_for_test();

        assert!(id_after_first_reconnect > id_before);
        assert!(id_after_second_reconnect > id_after_first_reconnect);
        session.shutdown();
    }

    #[test]
    fn reconnect_is_bounded_and_fails_visibly_when_binary_disappears() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let mut session = connect_to_fake(&f, CodexAppServerOptions::default());

        // Remove the binary so every reconnect attempt fails, proving the
        // loop is bounded rather than retrying forever.
        std::fs::remove_file(f.bin_dir.join("fake-app-server")).unwrap();

        for _ in 0..MAX_RECONNECT_ATTEMPTS {
            assert!(session.reconnect().is_err());
        }
        let err = session.reconnect().unwrap_err();
        assert!(err.to_string().contains("exceeded"));
    }

    #[test]
    fn missing_binary_produces_a_useful_error_instead_of_hanging() {
        let f = fixture();
        let result = CodexAppServerSession::connect(
            AppServerTarget::Stdio {
                executable: f.bin_dir.join("does-not-exist"),
                args: vec![],
            },
            &f.worktree,
            &[],
            &f.session_dir,
            CodexAppServerOptions::default(),
        );
        let Err(err) = result else {
            panic!("expected connect() to fail for a missing binary");
        };
        assert!(err.to_string().contains("is it installed"));
    }

    #[cfg(unix)]
    #[test]
    fn handshake_completes_over_unix_socket_transport() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let sock_path = f.session_dir.join("fake.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let reader = std::io::BufReader::new(stream);
            for line in reader.lines() {
                let line = line.unwrap();
                let value: Value = serde_json::from_str(&line).unwrap();
                match value.get("method").and_then(|m| m.as_str()) {
                    Some("initialize") => {
                        let id = value["id"].clone();
                        let response =
                            json!({"id": id, "result": {"userAgent": "fake-socket/1.0"}});
                        writeln!(writer, "{response}").unwrap();
                    }
                    Some("initialized") => break,
                    _ => {}
                }
            }
        });

        let session = CodexAppServerSession::connect(
            AppServerTarget::UnixSocket { path: sock_path },
            &f.worktree,
            &[],
            &f.session_dir,
            CodexAppServerOptions::default(),
        )
        .unwrap();

        assert_eq!(session.handshake_result()["userAgent"], "fake-socket/1.0");
        session.shutdown();
        server.join().unwrap();
    }
}
