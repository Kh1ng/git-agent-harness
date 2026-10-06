//! External contract of the `gah-mcp-server` binary: the tool list, the exact
//! HTTP request each tool forwards to the control plane, error mapping, and
//! the authenticated streamable-HTTP listener. Every test drives the real
//! binary as an MCP client would and observes a fake control plane.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOL_NAMES: [&str; 24] = [
    "gah_info",
    "gah_cli_router",
    "gah_status",
    "gah_quota",
    "gah_usage_rollup",
    "gah_doctor",
    "gah_report",
    "gah_profiles",
    "gah_work_history",
    "gah_sync",
    "gah_ledger_summary",
    "gah_ledger_clear_attempts",
    "gah_availability",
    "gah_availability_clear",
    "gah_hold",
    "gah_hold_set",
    "gah_hold_clear",
    "gah_events",
    "gah_controller_activity",
    "gah_loop_status",
    "gah_dispatch",
    "gah_route_approvals",
    "gah_route_approval_grant",
    "gah_route_approval_revoke",
];

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    /// Path and query exactly as sent.
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A control plane that records every request and answers each with the
/// currently configured status and body.
struct FakeControlPlane {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    reply: Arc<Mutex<(u16, String)>>,
}

impl FakeControlPlane {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let reply = Arc::new(Mutex::new((200, r#"{"ok":true}"#.to_string())));
        let (recorded, configured) = (requests.clone(), reply.clone());
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else { continue };
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                recorded.lock().unwrap().push(request);
                let (status, body) = configured.lock().unwrap().clone();
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            url,
            requests,
            reply,
        }
    }

    fn reply_with(&self, status: u16, body: &str) {
        *self.reply.lock().unwrap() = (status, body.to_string());
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    fn last(&self) -> Recorded {
        self.requests().pop().expect("a forwarded request")
    }
}

fn read_request(socket: &mut TcpStream) -> Option<Recorded> {
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut raw = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let count = socket.read(&mut buf).ok()?;
        if count == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..count]);
        let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&raw[..end]).to_string();
        let mut lines = head.lines();
        let mut request_line = lines.next()?.split(' ');
        let (method, target) = (request_line.next()?, request_line.next()?);
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_lowercase(), value.trim().to_string()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(key, _)| key == "content-length")
            .map_or(0, |(_, value)| value.parse().unwrap());
        if raw.len() >= end + 4 + length {
            return Some(Recorded {
                method: method.to_string(),
                target: target.to_string(),
                headers,
                body: String::from_utf8_lossy(&raw[end + 4..end + 4 + length]).to_string(),
            });
        }
    }
}

fn server_command(envs: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gah-mcp-server"));
    for name in [
        "GAH_SERVER_URL",
        "GAH_SERVER_TOKEN",
        "GAH_PROFILE",
        "GAH_MCP_HOST",
        "GAH_MCP_PORT",
    ] {
        command.env_remove(name);
    }
    command.envs(envs.iter().copied());
    command
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "gah-mcp-test", "version": "0.0.0" },
    })
}

/// An MCP client connected to the binary over stdio.
struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl StdioClient {
    fn connect(envs: &[(&str, &str)]) -> Self {
        let mut child = server_command(envs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut client = Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 0,
        };
        let initialized = client.request("initialize", initialize_params());
        assert_eq!(initialized["serverInfo"]["name"], "gah");
        client.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        client
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns its `result`.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert_ne!(
                self.stdout.read_line(&mut line).unwrap(),
                0,
                "the server closed stdout before answering {method}"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                assert!(message.get("error").is_none(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(result: &Value) -> &str {
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(result["content"][0]["type"], "text");
    result["content"][0]["text"].as_str().unwrap()
}

fn is_error(result: &Value) -> bool {
    result["isError"] == json!(true)
}

fn manifest_request_schemas() -> Value {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/packages/contracts/src/cli-capabilities.manifest.json"
        ))
        .unwrap(),
    )
    .unwrap();
    manifest["request_schemas"].clone()
}

#[test]
fn lists_the_same_24_tools_with_their_titles_and_input_schemas() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &control_plane.url)]);
    let listed = client.request("tools/list", json!({}));
    let tools = listed["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOL_NAMES);
    let tool = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
    for tool in tools {
        assert!(!tool["title"].as_str().unwrap().is_empty(), "{tool}");
        assert!(!tool["description"].as_str().unwrap().is_empty(), "{tool}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }

    let expected_metadata = [
        ("gah_info", "GAH server info", "Identify the connected GAH control-plane node and API version."),
        ("gah_cli_router", "GAH CLI router status", "Read-only snapshot of the CLI Proxy API connection status, settings, quotas, and allowed models."),
        ("gah_status", "GAH status", "Full status snapshot for a profile: merge requests, blockers, availability, ledger summary."),
        ("gah_quota", "GAH quota snapshot", "Usage/quota snapshot for a profile over a time window."),
        ("gah_usage_rollup", "GAH usage rollup", "Actual manager-chat usage by day, backend, and model; use days=30 for a monthly view."),
        ("gah_doctor", "GAH doctor", "Run readiness checks for a profile (auth, config, backend availability)."),
        ("gah_report", "GAH report", "Aggregate usage/cost/success-rate report, optionally grouped by backend or model."),
        ("gah_profiles", "List GAH profiles", "List all configured GAH profiles."),
        ("gah_work_history", "Work item ledger history", "Full chronological ledger history (all attempts) for one work item."),
        ("gah_sync", "GAH sync", "Classified open (and recently resolved) merge requests/pull requests for a profile."),
        ("gah_ledger_summary", "GAH ledger summary", "Aggregate ledger counts (success/fail, by mode/backend/model, token usage) over a window."),
        ("gah_ledger_clear_attempts", "Clear ledger attempts", "Append a tombstone ledger entry so a stuck work_id becomes dispatchable again."),
        ("gah_availability", "GAH availability", "Durable backend/model availability state, global (not per-profile)."),
        ("gah_availability_clear", "Clear availability override", "Override a stale unavailable record once the backend is confirmed healthy again."),
        ("gah_hold", "List review holds", "Work IDs currently under an out-of-band manager review hold for a profile."),
        ("gah_hold_set", "Set a review hold", "Mark a work_id as under active out-of-band manager review; gah's auto-merge loop will skip it."),
        ("gah_hold_clear", "Clear a review hold", "Release a previously set review hold on a work_id."),
        ("gah_events", "GAH events", "Recent controller and dispatch events for a profile."),
        ("gah_controller_activity", "GAH controller activity", "Summarized agent/controller activity for a profile."),
        ("gah_loop_status", "GAH loop status", "Report whether the autonomous GAH loop is running for a profile."),
        ("gah_dispatch", "Dispatch a GAH job", "Submit a dispatch as a fleet session and wait for its terminal push event by default. Set waitForCompletion=false to return immediately with the running session."),
        ("gah_route_approvals", "GAH paid-route approvals", "List pending and active paid-route approval requests for a profile (state, exact scope, consumption)."),
        ("gah_route_approval_grant", "GAH paid-route approval grant", "Grant one exact paid backend/model route for one work item. The scope must match the pending request exactly — it cannot be broadened here."),
        ("gah_route_approval_revoke", "GAH paid-route approval revoke", "Revoke a previously granted paid-route approval for one exact scope."),
    ];
    for (name, title, desc) in expected_metadata {
        assert_eq!(tool(name)["title"], title, "{name}");
        assert_eq!(tool(name)["description"], desc, "{name}");
    }
    // Tools without arguments advertise an empty object.
    for name in [
        "gah_info",
        "gah_cli_router",
        "gah_profiles",
        "gah_availability",
    ] {
        assert_eq!(
            tool(name)["inputSchema"],
            json!({ "type": "object", "properties": {} }),
            "{name}"
        );
    }

    // HTTP-only tools keep their hand-written schemas.
    let profile = json!({
        "type": "string",
        "description": "GAH profile name; defaults to GAH_PROFILE / \"gah\"",
    });
    assert_eq!(
        tool("gah_usage_rollup")["inputSchema"],
        json!({
            "type": "object",
            "properties": {
                "profile": profile,
                "days": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 90,
                    "default": 30,
                    "description": "Number of days to include (1-90).",
                },
            },
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#",
        })
    );
    for name in ["gah_hold", "gah_loop_status"] {
        assert_eq!(
            tool(name)["inputSchema"]["properties"],
            json!({ "profile": profile }),
            "{name}"
        );
    }
    assert_eq!(
        tool("gah_controller_activity")["inputSchema"]["properties"],
        json!({
            "profile": profile,
            "since": { "type": "string", "description": "e.g. \"24h\" or \"7d\"" },
        })
    );

    // CLI-backed tools are derived from the capability manifest, property
    // for property.
    let schemas = manifest_request_schemas();
    for (name, operation_id) in [
        ("gah_status", "status.get"),
        ("gah_quota", "quota.snapshot"),
        ("gah_doctor", "doctor.validate"),
        ("gah_report", "report.generate"),
        ("gah_work_history", "ledger.work"),
        ("gah_sync", "sync.classify"),
        ("gah_ledger_summary", "ledger.summary"),
        ("gah_ledger_clear_attempts", "ledger.clear_attempts"),
        ("gah_availability_clear", "availability.clear"),
        ("gah_hold_set", "hold.set"),
        ("gah_hold_clear", "hold.clear"),
        ("gah_events", "events.list"),
        ("gah_dispatch", "dispatch.run"),
        ("gah_route_approvals", "route_approval.list"),
        ("gah_route_approval_grant", "route_approval.grant"),
        ("gah_route_approval_revoke", "route_approval.revoke"),
    ] {
        let schema = &tool(name)["inputSchema"];
        let source = &schemas[operation_id];
        assert_eq!(schema["additionalProperties"], json!(false), "{name}");
        assert_eq!(
            schema["properties"], source["properties"],
            "{name} must mirror {operation_id}"
        );
    }
    let dispatch = &tool("gah_dispatch")["inputSchema"];
    assert_eq!(dispatch["required"], json!(["mode", "repo"]));
    assert_eq!(
        dispatch["properties"]["waitForCompletion"]["default"],
        json!(true)
    );
    assert_eq!(
        dispatch["properties"]["waitTimeoutSeconds"]["default"],
        json!(3600)
    );
    assert_eq!(
        tool("gah_route_approval_grant")["inputSchema"]["required"],
        json!(["backend", "profile", "work_id"])
    );
    assert!(tool("gah_status")["inputSchema"].get("required").is_none());
}

#[test]
fn every_tool_forwards_its_exact_http_request() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[
        ("GAH_SERVER_URL", &format!("{}/", control_plane.url)),
        ("GAH_SERVER_TOKEN", "test-token"),
        ("GAH_PROFILE", "fixture"),
    ]);
    let scope = json!({
        "profile": "real", "work_id": "#653", "backend": "opencode",
        "backend_instance": "paid-a", "model": "provider/model",
    });
    // (tool, arguments, method, path and query, JSON body)
    let cases: Vec<(&str, Value, &str, &str, Option<Value>)> = vec![
        ("gah_info", json!({}), "GET", "/api/info", None),
        ("gah_cli_router", json!({}), "GET", "/api/cli-router", None),
        (
            "gah_status",
            json!({}),
            "GET",
            "/api/status?profile=fixture",
            None,
        ),
        (
            "gah_status",
            json!({ "profile": "a b" }),
            "GET",
            "/api/status?profile=a+b",
            None,
        ),
        (
            "gah_quota",
            json!({ "profile": "gah", "since": "7d" }),
            "GET",
            "/api/quota?profile=gah&since=7d",
            None,
        ),
        (
            "gah_quota",
            json!({}),
            "GET",
            "/api/quota?profile=fixture",
            None,
        ),
        (
            "gah_usage_rollup",
            json!({ "profile": "gah", "days": 7 }),
            "GET",
            "/api/usage/rollup?profile=gah&days=7",
            None,
        ),
        (
            "gah_usage_rollup",
            json!({ "days": 30.0 }),
            "GET",
            "/api/usage/rollup?profile=fixture&days=30",
            None,
        ),
        // days defaults to the 30-day monthly view.
        (
            "gah_usage_rollup",
            json!({}),
            "GET",
            "/api/usage/rollup?profile=fixture&days=30",
            None,
        ),
        (
            "gah_doctor",
            json!({}),
            "GET",
            "/api/doctor?profile=fixture",
            None,
        ),
        // Reports span every profile unless one is named.
        ("gah_report", json!({}), "GET", "/api/report", None),
        (
            "gah_report",
            json!({ "profile": "gah", "since": "7d", "groupBy": "model" }),
            "GET",
            "/api/report?profile=gah&since=7d&groupBy=model",
            None,
        ),
        ("gah_profiles", json!({}), "GET", "/api/profiles", None),
        (
            "gah_work_history",
            json!({ "work_id": "#12/a" }),
            "GET",
            "/api/work/%2312%2Fa",
            None,
        ),
        (
            "gah_sync",
            json!({}),
            "GET",
            "/api/sync?profile=fixture",
            None,
        ),
        (
            "gah_ledger_summary",
            json!({}),
            "GET",
            "/api/ledger/summary",
            None,
        ),
        (
            "gah_ledger_summary",
            json!({ "profile": "gah", "since": "24h", "groupBy": "backend" }),
            "GET",
            "/api/ledger/summary?profile=gah&since=24h&groupBy=backend",
            None,
        ),
        (
            "gah_ledger_clear_attempts",
            json!({ "profile": "gah", "work_id": "519", "dry_run": true }),
            "POST",
            "/api/ledger/clear-attempts",
            Some(json!({ "profile": "gah", "workId": "519", "dryRun": true })),
        ),
        (
            "gah_availability",
            json!({}),
            "GET",
            "/api/availability",
            None,
        ),
        // availability.clear declares no request properties, so nothing a
        // client sends is forwarded.
        (
            "gah_availability_clear",
            json!({ "backend": "codex" }),
            "POST",
            "/api/availability/clear",
            Some(json!({})),
        ),
        (
            "gah_hold",
            json!({}),
            "GET",
            "/api/hold?profile=fixture",
            None,
        ),
        (
            "gah_hold_set",
            json!({ "work_id": "532", "reason": "manual review" }),
            "POST",
            "/api/hold/set",
            Some(json!({ "profile": "fixture", "workId": "532", "reason": "manual review" })),
        ),
        (
            "gah_hold_set",
            json!({ "profile": "gah", "work_id": "532" }),
            "POST",
            "/api/hold/set",
            Some(json!({ "profile": "gah", "workId": "532" })),
        ),
        (
            "gah_hold_clear",
            json!({ "profile": "gah", "work_id": "532" }),
            "POST",
            "/api/hold/clear",
            Some(json!({ "profile": "gah", "workId": "532" })),
        ),
        (
            "gah_events",
            json!({ "since": "24h" }),
            "GET",
            "/api/events?profile=fixture&since=24h",
            None,
        ),
        (
            "gah_controller_activity",
            json!({ "profile": "gah", "since": "7d" }),
            "GET",
            "/api/controller-activity?profile=gah&since=7d",
            None,
        ),
        (
            "gah_loop_status",
            json!({}),
            "GET",
            "/api/loop/status?profile=fixture",
            None,
        ),
        (
            "gah_dispatch",
            json!({
                "profile": "gah", "providerKind": "github", "instanceId": "local",
                "repo": "Kh1ng/git-agent-harness", "mode": "fix", "mr": "1100",
                "backend": "claude", "model": "sonnet", "retries": 1, "dryRun": true,
                "notAnOption": "dropped",
            }),
            "POST",
            "/api/dispatch",
            Some(json!({
                "profile": "gah", "providerKind": "github", "instanceId": "local",
                "repo": "Kh1ng/git-agent-harness", "mode": "fix", "mr": "1100",
                "backend": "claude", "model": "sonnet", "retries": 1, "dryRun": true,
                "waitForCompletion": true, "waitTimeoutSeconds": 3600,
            })),
        ),
        (
            "gah_dispatch",
            json!({ "repo": "o/r", "mode": "improve", "waitForCompletion": false }),
            "POST",
            "/api/dispatch",
            Some(json!({
                "profile": "fixture", "repo": "o/r", "mode": "improve",
                "waitForCompletion": false, "waitTimeoutSeconds": 3600,
            })),
        ),
        (
            "gah_route_approvals",
            json!({ "profile": "real" }),
            "GET",
            "/api/route-approvals?profile=real",
            None,
        ),
        (
            "gah_route_approval_grant",
            scope.clone(),
            "POST",
            "/api/route-approvals/grant",
            Some(json!({
                "profile": "real", "work_id": "#653", "backend": "opencode",
                "backend_instance": "paid-a", "model": "provider/model", "confirm": true,
            })),
        ),
        (
            "gah_route_approval_revoke",
            json!({ "profile": "real", "work_id": "#653", "backend": "opencode" }),
            "POST",
            "/api/route-approvals/revoke",
            Some(json!({
                "profile": "real", "work_id": "#653", "backend": "opencode",
                "backend_instance": null, "model": null, "confirm": true,
            })),
        ),
    ];
    let covered: std::collections::BTreeSet<&str> = cases.iter().map(|case| case.0).collect();
    assert_eq!(
        covered,
        TOOL_NAMES.into_iter().collect(),
        "every tool has a forwarding case"
    );

    for (index, (name, arguments, method, target, body)) in cases.iter().enumerate() {
        let result = client.call(name, arguments.clone());
        assert!(!is_error(&result), "{name}: {result}");
        assert_eq!(text(&result), "{\n  \"ok\": true\n}", "{name}");
        let requests = control_plane.requests();
        assert_eq!(
            requests.len(),
            index + 1,
            "{name} sends exactly one request"
        );
        let request = &requests[index];
        assert_eq!(request.method, *method, "{name}");
        assert_eq!(request.target, *target, "{name}");
        assert_eq!(
            request.header("authorization"),
            Some("Bearer test-token"),
            "{name}"
        );
        match body {
            Some(body) => {
                assert_eq!(
                    &serde_json::from_str::<Value>(&request.body).unwrap(),
                    body,
                    "{name}"
                );
                assert_eq!(request.header("content-type"), Some("application/json"));
            }
            None => assert_eq!(request.body, "", "{name}"),
        }
    }

    // Every mutation carries its own valid Idempotency-Key; reads carry none.
    let requests = control_plane.requests();
    let mut keys = std::collections::BTreeSet::new();
    for request in &requests {
        match (request.method.as_str(), request.header("idempotency-key")) {
            ("POST", Some(key)) => {
                assert!((16..=128).contains(&key.len()), "{key}");
                assert!(key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
                assert!(keys.insert(key.to_string()), "keys are never reused");
            }
            ("GET", None) => {}
            other => panic!(
                "{} {}: unexpected key state {other:?}",
                request.method, request.target
            ),
        }
    }
    assert_eq!(keys.len(), 9);
}

#[test]
fn defaults_to_the_gah_profile_and_sends_no_credentials_without_a_token() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &control_plane.url)]);
    let result = client.call("gah_status", json!({}));
    assert!(!is_error(&result), "{result}");
    let request = control_plane.last();
    assert_eq!(request.target, "/api/status?profile=gah");
    assert_eq!(request.header("authorization"), None);
}

#[test]
fn success_text_is_the_indented_response_in_the_servers_key_order() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &control_plane.url)]);
    control_plane.reply_with(200, r##"[{"work_id":"#653","approved":false}]"##);
    let result = client.call("gah_route_approvals", json!({ "profile": "real" }));
    assert_eq!(
        text(&result),
        "[\n  {\n    \"work_id\": \"#653\",\n    \"approved\": false\n  }\n]"
    );

    // Trailing whitespace is preserved for non-JSON text.
    control_plane.reply_with(200, "done\n");
    let text_result = client.call("gah_route_approvals", json!({ "profile": "real" }));
    assert_eq!(text(&text_result), "\"done\\n\"");
}

#[test]
fn failures_surface_the_servers_message_and_status_without_a_retry() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &control_plane.url)]);

    control_plane.reply_with(
        409,
        r#"{"message":"Operation already accepted. Refresh status."}"#,
    );
    let conflict = client.call("gah_hold_clear", json!({ "work_id": "532" }));
    assert!(is_error(&conflict));
    assert_eq!(
        text(&conflict),
        "Operation already accepted. Refresh status. (HTTP 409)"
    );
    assert_eq!(
        control_plane.requests().len(),
        1,
        "a conflict reaches the caller without an automatic retry"
    );

    // A body that is not a JSON message is passed through as-is.
    control_plane.reply_with(502, "upstream down");
    let bad_gateway = client.call("gah_status", json!({}));
    assert!(is_error(&bad_gateway));
    assert_eq!(text(&bad_gateway), "upstream down (HTTP 502)");

    control_plane.reply_with(502, "upstream down\n");
    let bad_gateway_nl = client.call("gah_status", json!({}));
    assert!(is_error(&bad_gateway_nl));
    assert_eq!(text(&bad_gateway_nl), "upstream down\n (HTTP 502)");

    control_plane.reply_with(500, "");
    let empty = client.call("gah_info", json!({}));
    assert!(is_error(&empty));
    assert_eq!(text(&empty), "HTTP 500 (HTTP 500)");
    assert_eq!(control_plane.requests().len(), 4);
}

#[test]
fn rejected_arguments_never_reach_the_control_plane() {
    let control_plane = FakeControlPlane::start();
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &control_plane.url)]);
    for (name, arguments) in [
        ("gah_hold_set", json!({ "profile": "gah" })),
        ("gah_hold_set", json!({ "work_id": 532 })),
        ("gah_usage_rollup", json!({ "days": 91 })),
        ("gah_usage_rollup", json!({ "days": 0 })),
        ("gah_usage_rollup", json!({ "days": 30.5 })),
        ("gah_dispatch", json!({ "repo": "o/r" })),
    ] {
        let result = client.call(name, arguments.clone());
        assert!(is_error(&result), "{name} {arguments}: {result}");
        assert!(
            text(&result).contains(&format!("Invalid arguments for tool {name}")),
            "{result}"
        );
    }
    assert!(control_plane.requests().is_empty());
}

#[test]
fn an_unreachable_control_plane_is_a_tool_error() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let mut client = StdioClient::connect(&[("GAH_SERVER_URL", &url)]);
    let result = client.call("gah_info", json!({}));
    assert!(is_error(&result), "{result}");
    assert!(!text(&result).is_empty());
}

/// The binary serving streamable HTTP on an ephemeral loopback port.
struct HttpServer {
    child: Child,
    port: u16,
}

impl HttpServer {
    fn start(envs: &[(&str, &str)]) -> Self {
        let mut child = server_command(envs)
            .args(["--http", "--port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut announcement = String::new();
        BufReader::new(child.stderr.take().unwrap())
            .read_line(&mut announcement)
            .unwrap();
        let port = announcement
            .trim()
            .strip_prefix("gah-mcp-server listening on http://127.0.0.1:")
            .and_then(|rest| rest.strip_suffix("/mcp"))
            .unwrap_or_else(|| panic!("unexpected announcement: {announcement:?}"))
            .parse()
            .unwrap();
        Self { child, port }
    }

    /// POSTs one JSON-RPC message to the MCP endpoint.
    fn post(&self, extra_headers: &[(&str, &str)], message: &Value) -> HttpReply {
        let body = message.to_string();
        let mut socket = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.port,
            body.len()
        );
        for (name, value) in extra_headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        socket
            .write_all(format!("{request}\r\n{body}").as_bytes())
            .unwrap();

        // Read until the JSON-RPC answer has arrived, or the server is done.
        let mut raw = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let reply = HttpReply::parse(&raw);
            if reply.as_ref().is_some_and(|reply| reply.message.is_some()) {
                return reply.unwrap();
            }
            match socket.read(&mut buf) {
                Ok(0) => return reply.expect("an HTTP response"),
                Ok(count) => raw.extend_from_slice(&buf[..count]),
                Err(error) => panic!("no answer from the MCP listener: {error}"),
            }
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpReply {
    status: u16,
    headers: Vec<(String, String)>,
    /// The JSON-RPC response, whether sent as JSON or as an SSE event.
    message: Option<Value>,
}

impl HttpReply {
    fn parse(raw: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(raw);
        let (head, body) = text.split_once("\r\n\r\n")?;
        let mut lines = head.lines();
        let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_lowercase(), value.trim().to_string()))
            .collect();
        let message = body
            .lines()
            .map(|line| line.strip_prefix("data:").unwrap_or(line).trim())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|value| value.get("jsonrpc").is_some());
        Some(Self {
            status,
            headers,
            message,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[test]
fn streamable_http_serves_tools_to_an_authenticated_client() {
    let control_plane = FakeControlPlane::start();
    let server = HttpServer::start(&[
        ("GAH_SERVER_URL", &control_plane.url),
        ("GAH_SERVER_TOKEN", "listener-token"),
        ("GAH_PROFILE", "fixture"),
    ]);
    let auth = ("Authorization", "Bearer listener-token");

    let initialized = server.post(
        &[auth],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params() }),
    );
    assert_eq!(initialized.status, 200);
    let message = initialized.message.as_ref().expect("an initialize result");
    assert_eq!(message["result"]["serverInfo"]["name"], "gah");
    let mut headers = vec![auth, ("MCP-Protocol-Version", "2025-06-18")];
    let session = initialized.header("mcp-session-id").map(str::to_string);
    if let Some(session) = &session {
        headers.push(("Mcp-Session-Id", session));
    }
    let acknowledged = server.post(
        &headers,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert_eq!(acknowledged.status, 202);

    let listed = server.post(
        &headers,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
    );
    assert_eq!(listed.status, 200);
    let listed = listed.message.expect("a tools/list result");
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOL_NAMES);

    let called = server.post(
        &headers,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "gah_hold_set", "arguments": { "work_id": "532" } },
        }),
    );
    assert_eq!(called.status, 200);
    let called = called.message.expect("a tools/call result");
    assert_eq!(text(&called["result"]), "{\n  \"ok\": true\n}");
    let forwarded = control_plane.last();
    assert_eq!(
        (forwarded.method.as_str(), forwarded.target.as_str()),
        ("POST", "/api/hold/set")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&forwarded.body).unwrap(),
        json!({ "profile": "fixture", "workId": "532" })
    );
    assert_eq!(
        forwarded.header("authorization"),
        Some("Bearer listener-token")
    );
}

#[test]
fn streamable_http_rejects_requests_without_the_bearer_token() {
    let control_plane = FakeControlPlane::start();
    let server = HttpServer::start(&[
        ("GAH_SERVER_URL", &control_plane.url),
        ("GAH_SERVER_TOKEN", "listener-token"),
    ]);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "gah_hold_set", "arguments": { "work_id": "532" } },
    });
    let initialize =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params() });
    for headers in [
        vec![],
        vec![("Authorization", "Bearer wrong-token")],
        vec![("Authorization", "listener-token")],
        vec![("Authorization", "Basic listener-token")],
    ] {
        for message in [&initialize, &call] {
            let reply = server.post(&headers, message);
            assert_eq!(reply.status, 401, "{headers:?}");
            assert_eq!(reply.header("www-authenticate"), Some("Bearer"));
            assert!(reply.message.is_none());
        }
    }
    assert!(
        control_plane.requests().is_empty(),
        "an unauthenticated client never reaches the control plane"
    );
}

#[test]
fn the_http_listener_refuses_to_start_without_a_token_or_with_a_non_literal_host() {
    let without_token = server_command(&[])
        .args(["--http", "--port", "0"])
        .output()
        .unwrap();
    assert!(!without_token.status.success());
    assert!(String::from_utf8_lossy(&without_token.stderr).contains("GAH_SERVER_TOKEN"));

    let named_host = server_command(&[("GAH_SERVER_TOKEN", "listener-token")])
        .args(["--http", "--port", "0", "--host", "localhost"])
        .output()
        .unwrap();
    assert!(!named_host.status.success());
    assert!(String::from_utf8_lossy(&named_host.stderr).contains("invalid bind host"));
}

#[test]
fn outbound_requests_ignore_curl_user_configuration() {
    let control_plane = FakeControlPlane::start();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".curlrc"), "retry = 3\nretry-all-errors").unwrap();
    let mut client = StdioClient::connect(&[
        ("GAH_SERVER_URL", &control_plane.url),
        ("HOME", home.path().to_str().unwrap()),
        ("CURL_HOME", home.path().to_str().unwrap()),
    ]);
    control_plane.reply_with(500, "transient error");
    let result = client.call("gah_status", json!({}));
    assert!(is_error(&result));

    // If .curlrc was loaded, curl would have retried 3 times, meaning 4 requests total.
    // With -q, it should ignore .curlrc and only send 1 request.
    assert_eq!(
        control_plane.requests().len(),
        1,
        "curl should not retry based on .curlrc"
    );
}
