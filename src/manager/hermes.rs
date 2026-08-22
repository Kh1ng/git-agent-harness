//! Hermes-backed `ManagerSession` adapter.
//!
//! Hermes exposes the ACP server on stdio (`hermes acp`). That gives us a
//! structured session lifecycle with real session IDs, prompt turns, prompt
//! cancellation, and replay/resume support without scraping presentation
//! prose. This adapter keeps the transport synchronous and object-safe so it
//! can satisfy the shared `ManagerSession` trait directly.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const ACP_PROTOCOL_VERSION: u64 = 1;
const DEFAULT_HERMES_PROFILE: &str = "gah-manager";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HermesAuthState {
    Unknown,
    LoggedOut,
    LoggedIn,
    Error(String),
}

impl fmt::Display for HermesAuthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("unknown"),
            Self::LoggedOut => f.write_str("logged_out"),
            Self::LoggedIn => f.write_str("logged_in"),
            Self::Error(message) => write!(f, "error({message})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesDiscovery {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub auth_state: HermesAuthState,
}

#[derive(Debug)]
struct HermesSessionState {
    provider_session_id: String,
    pending_updates: Vec<SessionUpdate>,
    status: SessionStatus,
}

#[derive(Debug)]
struct HermesTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    supports_load_session: bool,
    supports_resume: bool,
}

impl HermesTransport {
    fn spawn(executable: &Path) -> Result<Self> {
        let mut cmd = Command::new(executable);
        // Hermes's ACP server is launched as `hermes acp` in the upstream
        // docs; we keep the same hard-coded profile used by the TS adapter.
        cmd.arg("-p").arg(DEFAULT_HERMES_PROFILE).arg("acp");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        crate::runner::process::prepare_process_group(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("launching Hermes ACP from {}", executable.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Hermes ACP child did not provide stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Hermes ACP child did not provide stdout"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            supports_load_session: false,
            supports_resume: false,
        })
    }

    fn write_json(&mut self, value: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, value)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_json(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn cancel(&mut self, session_id: &str) -> Result<()> {
        self.send_notification(
            "session/cancel",
            json!({
                "sessionId": session_id,
            }),
        )
    }

    fn handle_session_update(
        sessions: &mut HashMap<GahSessionId, HermesSessionState>,
        params: Option<&Value>,
    ) -> Result<()> {
        let Some(params) = params else {
            return Ok(());
        };
        let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
            return Ok(());
        };
        let Some(update) = params.get("update") else {
            return Ok(());
        };

        let chunks = extract_message_chunks(update);
        if chunks.is_empty() {
            return Ok(());
        }
        if let Some(session) = sessions
            .values_mut()
            .find(|session| session.provider_session_id == session_id)
        {
            session
                .pending_updates
                .extend(chunks.into_iter().map(SessionUpdate::MessageChunk));
        }
        Ok(())
    }
}

fn rpc_request(
    transport: &mut HermesTransport,
    sessions: &mut HashMap<GahSessionId, HermesSessionState>,
    method: &str,
    params: Value,
) -> Result<Value> {
    let id = transport.next_id;
    transport.next_id += 1;
    transport.write_json(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))?;

    loop {
        let mut line = String::new();
        let bytes = transport.stdout.read_line(&mut line)?;
        if bytes == 0 {
            return Err(anyhow!(
                "Hermes ACP process exited while waiting for {method}"
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing Hermes ACP line: {trimmed}"))?;

        if is_response_for(&message, id) {
            return decode_json_rpc_response(message);
        }
        if let Some(request_method) = message.get("method").and_then(Value::as_str) {
            if let Some(request_id) = message.get("id").and_then(Value::as_u64) {
                if request_method == "session/request_permission" {
                    let selected = message
                        .get("params")
                        .and_then(|params| params.get("options"))
                        .and_then(Value::as_array)
                        .and_then(|options| {
                            options.iter().find_map(|option| {
                                let kind = option.get("kind").and_then(Value::as_str)?;
                                let option_id = option.get("optionId").and_then(Value::as_str)?;
                                ((kind == "reject_once") || (kind == "reject_always"))
                                    .then(|| option_id.to_owned())
                            })
                        });
                    let result = selected
                        .map(|option_id| {
                            json!({"outcome": {"outcome": "selected", "optionId": option_id}})
                        })
                        .unwrap_or_else(|| json!({"outcome": {"outcome": "cancelled"}}));
                    transport.write_json(&json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": result,
                    }))?;
                    continue;
                }

                transport.write_json(&json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32601,
                        "message": format!("unsupported client request {request_method}"),
                    }
                }))?;
                continue;
            } else if request_method == "session/update" {
                HermesTransport::handle_session_update(sessions, message.get("params"))?;
                continue;
            }
        }
    }
}

fn is_response_for(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn decode_json_rpc_response(message: Value) -> Result<Value> {
    if let Some(error) = message.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Hermes ACP request failed");
        return Err(anyhow!("{message} (code {code})"));
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

fn extract_message_chunks(update: &Value) -> Vec<String> {
    let Some(kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
        return vec![];
    };
    match kind {
        "agent_message_chunk" | "user_message_chunk" | "thought_chunk" => extract_texts(update),
        "usage_update"
        | "tool_call"
        | "tool_call_update"
        | "available_commands_update"
        | "session_info_update" => vec![],
        _ => extract_texts(update),
    }
}

fn extract_texts(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        out.push(text.to_owned());
    }
    if let Some(content) = value.get("content") {
        match content {
            Value::String(text) => out.push(text.to_owned()),
            Value::Array(items) => {
                for item in items {
                    out.extend(extract_texts(item));
                }
            }
            Value::Object(_) => {
                out.extend(extract_texts(content));
            }
            _ => {}
        }
    }
    if let Some(chunks) = value.get("chunks").and_then(Value::as_array) {
        for chunk in chunks {
            out.extend(extract_texts(chunk));
        }
    }
    out
}

fn parse_version_from_output(output: &std::process::Output) -> Option<String> {
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn classify_auth_state(output: &std::process::Output) -> HermesAuthState {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let message = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{stderr}: {stdout}")
        };
        return HermesAuthState::Error(sanitize_auth_message(&message));
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lower = combined.to_lowercase();
    if lower.contains("logged in") || lower.contains("authenticated") {
        HermesAuthState::LoggedIn
    } else if lower.contains("not logged in")
        || lower.contains("logged out")
        || lower.contains("no credentials")
    {
        HermesAuthState::LoggedOut
    } else {
        HermesAuthState::Unknown
    }
}

fn sanitize_auth_message(message: &str) -> String {
    let mut words = Vec::new();
    for word in message.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if lower.contains("token")
            || lower.contains("secret")
            || lower.contains("apikey")
            || lower.contains("api-key")
            || lower.contains("password")
        {
            words.push("[redacted]".to_string());
        } else {
            words.push(word.to_string());
        }
    }
    words.join(" ")
}

pub fn discover(executable: impl AsRef<Path>) -> Result<HermesDiscovery> {
    let executable = executable.as_ref().to_path_buf();
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output))
        .and_then(|output| parse_version_from_output(&output));

    let auth_state = match Command::new(&executable).args(["status", "--all"]).output() {
        Ok(output) => classify_auth_state(&output),
        Err(error) => HermesAuthState::Error(sanitize_auth_message(&error.to_string())),
    };

    Ok(HermesDiscovery {
        executable,
        version,
        auth_state,
    })
}

pub struct HermesManagerSession {
    discovery: HermesDiscovery,
    transport: HermesTransport,
    sessions: HashMap<GahSessionId, HermesSessionState>,
    capabilities: SessionCapabilities,
}

impl HermesManagerSession {
    pub fn discover(executable: impl AsRef<Path>) -> Result<HermesDiscovery> {
        discover(executable)
    }

    pub fn new(executable: impl AsRef<Path>) -> Result<Self> {
        let discovery = discover(executable.as_ref())?;
        let mut transport = HermesTransport::spawn(&discovery.executable)?;
        let mut temp_sessions = HashMap::new();
        let response = rpc_request(
            &mut transport,
            &mut temp_sessions,
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": false,
                        "writeTextFile": false
                    },
                    "terminal": false,
                    "auth": {
                        "terminal": false
                    }
                },
                "clientInfo": {
                    "name": "git-agent-harness",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        transport.supports_load_session = response
            .get("agentCapabilities")
            .and_then(|caps| caps.get("loadSession"))
            .is_some_and(|value| value != &Value::Bool(false));
        let session_capabilities = response
            .get("agentCapabilities")
            .and_then(|caps| caps.get("sessionCapabilities"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        transport.supports_resume = session_capabilities
            .get("resume")
            .is_some_and(|value| value != &Value::Bool(false))
            || transport.supports_load_session;
        let capabilities = SessionCapabilities {
            resume: transport.supports_load_session || transport.supports_resume,
            interrupt: true,
            inspect: false,
        };
        Ok(Self {
            discovery,
            transport,
            sessions: HashMap::new(),
            capabilities,
        })
    }

    pub fn discovery(&self) -> &HermesDiscovery {
        &self.discovery
    }

    fn state_mut(&mut self, session: &GahSessionId) -> Result<&mut HermesSessionState> {
        self.sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("unknown Hermes session {session}"))
    }

    fn session_id_from_gah(session: &GahSessionId) -> Result<String> {
        let raw = session.as_str();
        let suffix = raw
            .strip_prefix("gah:manager:")
            .ok_or_else(|| anyhow!("invalid Hermes GAH session id {raw}"))?;
        let (_, provider_session_id) = suffix
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("invalid Hermes GAH session id {raw}"))?;
        Ok(provider_session_id.to_owned())
    }

    fn record_session(&mut self, session: GahSessionId, provider_session_id: String) {
        self.sessions.insert(
            session,
            HermesSessionState {
                provider_session_id,
                pending_updates: Vec::new(),
                status: SessionStatus::Idle,
            },
        );
    }
}

impl ManagerSession for HermesManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let cwd = std::env::current_dir().context("resolving current directory for Hermes ACP")?;
        let mut ignored = HashMap::new();
        let response = rpc_request(
            &mut self.transport,
            &mut ignored,
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": [],
            }),
        )?;
        let provider_session_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Hermes ACP session/new response did not include sessionId"))?;
        let gah_session_id =
            GahSessionId::from_provider_session(&request.profile, &provider_session_id);
        self.record_session(gah_session_id.clone(), provider_session_id.clone());
        let _ = rpc_request(
            &mut self.transport,
            &mut self.sessions,
            "session/prompt",
            json!({
                "sessionId": provider_session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": request.instruction
                    }
                ]
            }),
        )
        .with_context(|| format!("prompting Hermes session {provider_session_id}"))?;
        Ok(gah_session_id)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.resume {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        let cwd = std::env::current_dir().context("resolving current directory for Hermes ACP")?;
        let provider_session_id = Self::session_id_from_gah(session)?;
        if !self.sessions.contains_key(session) {
            self.record_session(session.clone(), provider_session_id.clone());
        }
        let method = if self.transport.supports_resume {
            "session/resume"
        } else if self.transport.supports_load_session {
            "session/load"
        } else {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        };
        let _ = rpc_request(
            &mut self.transport,
            &mut self.sessions,
            method,
            json!({
                "sessionId": provider_session_id,
                "cwd": cwd,
                "mcpServers": [],
            }),
        )
        .with_context(|| format!("resuming Hermes session {provider_session_id}"))?;
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Idle;
        }
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let provider_session_id = Self::session_id_from_gah(session)?;
        if !self.sessions.contains_key(session) {
            self.record_session(session.clone(), provider_session_id.clone());
        }
        let _ = rpc_request(
            &mut self.transport,
            &mut self.sessions,
            "session/prompt",
            json!({
                "sessionId": provider_session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": message
                    }
                ]
            }),
        )
        .with_context(|| format!("prompting Hermes session {provider_session_id}"))?;
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Idle;
        }
        Ok(())
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        let state = self.state_mut(session)?;
        Ok(std::mem::take(&mut state.pending_updates))
    }

    fn interrupt(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.interrupt {
            return Err(UnsupportedCapability {
                capability: "interrupt",
            }
            .into());
        }
        let provider_session_id = Self::session_id_from_gah(session)?;
        self.transport.cancel(&provider_session_id)?;
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Terminated(TerminalStatus::Interrupted);
        }
        Ok(())
    }

    fn inspect(&mut self, _session: &GahSessionId) -> Result<SessionStatus> {
        Err(UnsupportedCapability {
            capability: "inspect",
        }
        .into())
    }

    fn terminal_status(&mut self, session: &GahSessionId) -> Result<Option<TerminalStatus>> {
        let state = self.state_mut(session)?;
        Ok(match &state.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}

impl Drop for HermesManagerSession {
    fn drop(&mut self) {
        let _ = self.transport.child.kill();
        let _ = self.transport.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::fake::FakeManagerSession;
    use crate::runner::backends::test_util::{fixture, make_fake_bin};

    fn make_json_rpc_hermes(dir: &Path, record_dir: &Path) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'hermes 1.2.3'
  exit 0
fi
if [ "$1" = "status" ]; then
  echo 'OpenAI Codex ✓ logged in'
  echo 'token=super-secret' >&2
  exit 0
fi

tmp="$(mktemp)"
cat > "$tmp" <<'PY'
import json
import os
import sys

record = os.environ["RECORD_PATH"]
with open(record, "a", encoding="utf-8") as fh:
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        fh.write(line + "\n")
        fh.flush()
        msg = json.loads(line)
        method = msg.get("method")
        req_id = msg.get("id")

        if method == "initialize":
            resp = {{
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {{
                    "protocolVersion": 1,
                    "agentCapabilities": {{
                        "loadSession": True,
                        "sessionCapabilities": {{
                            "resume": {{}}
                        }}
                    }},
                    "authMethods": []
                }}
            }}
            print(json.dumps(resp), flush=True)
        elif method == "session/new":
            resp = {{
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {{
                    "sessionId": "sess-new"
                }}
            }}
            print(json.dumps(resp), flush=True)
        elif method in ("session/resume", "session/load"):
            print(json.dumps({{"jsonrpc": "2.0", "id": req_id, "result": {{}} }}), flush=True)
        elif method == "session/prompt":
            session_id = msg["params"]["sessionId"]
            update = {{
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {{
                    "sessionId": session_id,
                    "update": {{
                        "sessionUpdate": "agent_message_chunk",
                        "content": {{"type": "text", "text": "reply: " + msg["params"]["prompt"][0]["text"]}}
                    }}
                }}
            }}
            print(json.dumps(update), flush=True)
            resp = {{
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {{"stopReason": "end_turn"}}
            }}
            print(json.dumps(resp), flush=True)
        elif method == "session/cancel":
            pass
        elif method == "session/request_permission":
            resp = {{
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {{"outcome": {{"outcome": "cancelled"}}}}
            }}
            print(json.dumps(resp), flush=True)
        else:
            print(json.dumps({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32601, "message": "unknown method"}}}}), flush=True)
PY
export RECORD_PATH='{record_path}'
exec python3 -u "$tmp" "$@"
"#,
            record_path = record_dir.join("requests.jsonl").display()
        );
        make_fake_bin(dir, "hermes", &body);
    }

    #[test]
    fn discover_reports_version_and_redacted_auth_state() {
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "hermes",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'hermes 1.2.3'; exit 0; fi\nif [ \"$1\" = \"status\" ]; then echo 'OpenAI Codex ✓ logged in'; echo 'token=super-secret' >&2; exit 0; fi\nexit 1\n",
        );

        let discovery = discover(f.bin_dir.join("hermes")).unwrap();
        assert_eq!(discovery.version.as_deref(), Some("hermes 1.2.3"));
        assert_eq!(discovery.auth_state, HermesAuthState::LoggedIn);
    }

    #[test]
    fn adapter_runs_prompt_and_streams_message_chunks() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);

        let mut session = HermesManagerSession::new(f.bin_dir.join("hermes")).unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        let updates = session.stream(&id).unwrap();
        assert_eq!(
            updates,
            vec![SessionUpdate::MessageChunk("reply: hello".into())]
        );
        assert_eq!(session.terminal_status(&id).unwrap(), None);
    }

    #[test]
    fn adapter_resume_and_interrupt_round_trip() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);

        let mut session = HermesManagerSession::new(f.bin_dir.join("hermes")).unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        session.resume(&id).unwrap();
        session.interrupt(&id).unwrap();
        assert_eq!(
            session.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Interrupted)
        );
    }

    #[test]
    fn contract_suite_can_run_against_the_fake_with_resume_off() {
        let mut session = FakeManagerSession::new(SessionCapabilities::default());
        crate::manager::contract::run_contract_suite(&mut session);
    }
}
