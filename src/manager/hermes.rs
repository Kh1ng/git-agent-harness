//! Hermes-backed `ManagerSession` adapter.
//!
//! Hermes exposes a structured ACP server (`hermes acp`) with session
//! creation, resume/load, prompt streaming, and structured usage
//! reporting. This adapter stays on that structured surface instead of
//! scraping prose output.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;

const ACP_CLIENT_NAME: &str = "gah";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesUsage {
    pub cached_read_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HermesAuthState {
    LoggedIn {
        provider_id: String,
        summary: String,
    },
    NotLoggedIn {
        provider_id: Option<String>,
        summary: String,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesDiscovery {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub load_session_supported: bool,
    pub resume_supported: bool,
    pub auth_state: HermesAuthState,
}

#[derive(Debug, Clone)]
struct HermesSessionState {
    pending: Vec<SessionUpdate>,
    status: SessionStatus,
    last_usage: Option<HermesUsage>,
}

#[derive(Debug, Clone)]
struct HermesInitializeProbe {
    version: Option<String>,
    load_session_supported: bool,
    resume_supported: bool,
}

#[derive(Debug, Clone)]
struct HermesSessionCreate {
    session_id: String,
}

#[derive(Debug, Clone)]
struct HermesPromptResult {
    stop_reason: Option<String>,
    usage: Option<HermesUsage>,
    updates: Vec<SessionUpdate>,
}

pub struct HermesManagerSession {
    executable: PathBuf,
    discovery: HermesDiscovery,
    capabilities: SessionCapabilities,
    sessions: HashMap<GahSessionId, HermesSessionState>,
}

impl HermesManagerSession {
    pub fn new() -> Self {
        Self::with_executable("hermes")
    }

    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        let executable = executable.into();
        let probe = probe_initialize(&executable).unwrap_or(HermesInitializeProbe {
            version: None,
            load_session_supported: false,
            resume_supported: false,
        });
        let discovery = HermesDiscovery::discover(&executable, probe.clone());
        let capabilities = SessionCapabilities {
            resume: probe.load_session_supported || probe.resume_supported,
            interrupt: true,
            inspect: true,
        };
        Self {
            executable,
            discovery,
            capabilities,
            sessions: HashMap::new(),
        }
    }

    pub fn discovery(&self) -> &HermesDiscovery {
        &self.discovery
    }

    pub fn last_usage(&self, session: &GahSessionId) -> Option<&HermesUsage> {
        self.sessions
            .get(session)
            .and_then(|state| state.last_usage.as_ref())
    }

    fn session_state_mut(&mut self, session: &GahSessionId) -> Result<&mut HermesSessionState> {
        self.sessions
            .get_mut(session)
            .ok_or_else(|| anyhow::anyhow!("unknown Hermes session {session}"))
    }

    fn current_worktree() -> Result<PathBuf> {
        std::env::current_dir().context("reading current working directory for Hermes ACP")
    }

    fn session_id(profile: &str, provider_session_id: &str) -> GahSessionId {
        GahSessionId(format!(
            "gah:manager:{profile}:hermes:{provider_session_id}"
        ))
    }

    fn provider_session_id(session: &GahSessionId) -> Result<&str> {
        session
            .as_str()
            .rsplit_once(":hermes:")
            .map(|(_, id)| id)
            .ok_or_else(|| anyhow::anyhow!("Hermes session id {session} is not a Hermes session"))
    }

    fn launch_acp_client(&self) -> Result<AcpClient> {
        AcpClient::spawn(&self.executable)
    }

    fn run_session_new(&self, cwd: &Path) -> Result<HermesSessionCreate> {
        let mut client = self.launch_acp_client()?;
        client.initialize()?;
        let response = client.session_new(cwd)?;
        client.finish()?;
        Ok(response)
    }

    fn resume_existing_session(&self, session: &GahSessionId, cwd: &Path) -> Result<()> {
        let provider_session_id = Self::provider_session_id(session)?;
        let mut client = self.launch_acp_client()?;
        client.initialize()?;
        if self.discovery.resume_supported {
            client.session_resume(provider_session_id, cwd)?;
        } else if self.discovery.load_session_supported {
            client.session_load(provider_session_id, cwd)?;
        } else {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        client.finish()?;
        Ok(())
    }

    fn prompt_session(
        &self,
        session: &GahSessionId,
        message: &str,
        cwd: &Path,
    ) -> Result<HermesPromptResult> {
        let provider_session_id = Self::provider_session_id(session)?;
        let mut client = self.launch_acp_client()?;
        client.initialize()?;
        if self.discovery.resume_supported {
            client.session_resume(provider_session_id, cwd)?;
        } else if self.discovery.load_session_supported {
            client.session_load(provider_session_id, cwd)?;
        } else {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        let result = client.session_prompt(provider_session_id, message)?;
        client.finish()?;
        Ok(result)
    }

    fn cancel_session(&self, session: &GahSessionId, cwd: &Path) -> Result<()> {
        let provider_session_id = Self::provider_session_id(session)?;
        let mut client = self.launch_acp_client()?;
        client.initialize()?;
        if self.discovery.resume_supported {
            client.session_resume(provider_session_id, cwd)?;
        } else if self.discovery.load_session_supported {
            client.session_load(provider_session_id, cwd)?;
        } else {
            return Err(UnsupportedCapability {
                capability: "interrupt",
            }
            .into());
        }
        client.session_cancel(provider_session_id)?;
        client.finish()?;
        Ok(())
    }

    fn store_turn(
        &mut self,
        session_id: GahSessionId,
        usage: Option<HermesUsage>,
        updates: Vec<SessionUpdate>,
        status: SessionStatus,
    ) {
        self.sessions.insert(
            session_id,
            HermesSessionState {
                pending: updates,
                status,
                last_usage: usage,
            },
        );
    }
}

impl Default for HermesManagerSession {
    fn default() -> Self {
        Self::new()
    }
}

impl HermesDiscovery {
    fn discover(executable: &Path, probe: HermesInitializeProbe) -> Self {
        let auth_state = discover_auth_state(executable).unwrap_or(HermesAuthState::Unknown);
        Self {
            executable: executable.to_path_buf(),
            version: probe.version,
            load_session_supported: probe.load_session_supported,
            resume_supported: probe.resume_supported,
            auth_state,
        }
    }
}

impl ManagerSession for HermesManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let cwd = Self::current_worktree()?;
        let session = self.run_session_new(&cwd)?;
        let session_id = Self::session_id(&request.profile, &session.session_id);
        let result = self.prompt_session(&session_id, &request.instruction, &cwd)?;
        let status = if result.stop_reason.as_deref() == Some("cancelled") {
            SessionStatus::Terminated(TerminalStatus::Interrupted)
        } else {
            SessionStatus::Idle
        };
        self.store_turn(session_id.clone(), result.usage, result.updates, status);
        Ok(session_id)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.resume {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        let cwd = Self::current_worktree()?;
        self.resume_existing_session(session, &cwd)?;
        self.sessions
            .entry(session.clone())
            .and_modify(|state| {
                state.status = SessionStatus::Idle;
            })
            .or_insert(HermesSessionState {
                pending: vec![],
                status: SessionStatus::Idle,
                last_usage: None,
            });
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let cwd = Self::current_worktree()?;
        let result = self.prompt_session(session, message, &cwd)?;
        let status = if result.stop_reason.as_deref() == Some("cancelled") {
            SessionStatus::Terminated(TerminalStatus::Interrupted)
        } else {
            SessionStatus::Idle
        };
        self.store_turn(session.clone(), result.usage, result.updates, status);
        Ok(())
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        let state = self.session_state_mut(session)?;
        Ok(std::mem::take(&mut state.pending))
    }

    fn interrupt(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.interrupt {
            return Err(UnsupportedCapability {
                capability: "interrupt",
            }
            .into());
        }
        let cwd = Self::current_worktree()?;
        self.cancel_session(session, &cwd)?;
        let state = self
            .sessions
            .entry(session.clone())
            .or_insert_with(|| HermesSessionState {
                pending: vec![],
                status: SessionStatus::Idle,
                last_usage: None,
            });
        state.status = SessionStatus::Terminated(TerminalStatus::Interrupted);
        Ok(())
    }

    fn inspect(&mut self, session: &GahSessionId) -> Result<SessionStatus> {
        if !self.capabilities.inspect {
            return Err(UnsupportedCapability {
                capability: "inspect",
            }
            .into());
        }
        Ok(self.session_state_mut(session)?.status.clone())
    }

    fn terminal_status(&mut self, session: &GahSessionId) -> Result<Option<TerminalStatus>> {
        let state = self.session_state_mut(session)?;
        Ok(match &state.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}

struct AcpClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
    next_id: u64,
    pending_updates: Vec<SessionUpdate>,
}

impl AcpClient {
    fn spawn(executable: &Path) -> Result<Self> {
        let mut command = Command::new(executable);
        if !matches!(
            executable.file_name().and_then(|name| name.to_str()),
            Some("hermes-acp" | "hermes_acp")
        ) {
            command.arg("acp");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("launching Hermes ACP from {}", executable.display()))?;
        let stdin = child.stdin.take().context("capturing Hermes ACP stdin")?;
        let stdout = child.stdout.take().context("capturing Hermes ACP stdout")?;
        let stderr = child.stderr.take().context("capturing Hermes ACP stderr")?;
        let stderr_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_clone = std::sync::Arc::clone(&stderr_buffer);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut buffer = stderr_clone.lock().expect("stderr buffer lock");
                        buffer.push_str(&line);
                        if buffer.len() > 64 * 1024 {
                            let keep_from = buffer.len() - 64 * 1024;
                            buffer.drain(..keep_from);
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            stderr: stderr_buffer,
            next_id: 0,
            pending_updates: vec![],
        })
    }

    fn finish(mut self) -> Result<()> {
        self.stdin.flush().ok();
        drop(self.stdin);
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        serde_json::to_writer(&mut self.stdin, &request)
            .context("serializing Hermes ACP request")?;
        self.stdin
            .write_all(b"\n")
            .context("terminating Hermes ACP request line")?;
        self.stdin.flush().context("flushing Hermes ACP request")?;
        self.read_until_response(id)
    }

    fn notification(&mut self, method: &str, params: Value) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        serde_json::to_writer(&mut self.stdin, &notification)
            .context("serializing Hermes ACP notification")?;
        self.stdin
            .write_all(b"\n")
            .context("terminating Hermes ACP notification line")?;
        self.stdin
            .flush()
            .context("flushing Hermes ACP notification")
    }

    fn initialize(&mut self) -> Result<HermesInitializeProbe> {
        let response = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": ACP_CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        let result = response
            .get("result")
            .and_then(Value::as_object)
            .context("Hermes ACP initialize response missing result")?;
        let version = result
            .get("agentInfo")
            .and_then(Value::as_object)
            .and_then(|agent| agent.get("version"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let load_session_supported = result
            .get("agentCapabilities")
            .and_then(Value::as_object)
            .and_then(|caps| caps.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let resume_supported = result
            .get("agentCapabilities")
            .and_then(Value::as_object)
            .and_then(|caps| caps.get("sessionCapabilities"))
            .and_then(Value::as_object)
            .and_then(|session_caps| session_caps.get("resume"))
            .is_some();
        Ok(HermesInitializeProbe {
            version,
            load_session_supported,
            resume_supported,
        })
    }

    fn session_new(&mut self, cwd: &Path) -> Result<HermesSessionCreate> {
        let response = self.request(
            "session/new",
            serde_json::json!({
                "cwd": cwd,
                "mcpServers": [],
            }),
        )?;
        let result = response
            .get("result")
            .and_then(Value::as_object)
            .context("Hermes ACP session/new response missing result")?;
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .context("Hermes ACP session/new response missing sessionId")?;
        Ok(HermesSessionCreate {
            session_id: session_id.to_string(),
        })
    }

    fn session_resume(&mut self, session_id: &str, cwd: &Path) -> Result<()> {
        let _ = self.request(
            "session/resume",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": [],
            }),
        )?;
        Ok(())
    }

    fn session_load(&mut self, session_id: &str, cwd: &Path) -> Result<()> {
        let _ = self.request(
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": [],
            }),
        )?;
        Ok(())
    }

    fn session_prompt(&mut self, session_id: &str, message: &str) -> Result<HermesPromptResult> {
        self.clear_updates();
        let response = self.request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": message,
                    }
                ],
            }),
        )?;
        let result = response
            .get("result")
            .and_then(Value::as_object)
            .context("Hermes ACP session/prompt response missing result")?;
        Ok(HermesPromptResult {
            stop_reason: result
                .get("stopReason")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            usage: result.get("usage").and_then(parse_usage),
            updates: self.take_updates(),
        })
    }

    fn clear_updates(&mut self) {
        self.pending_updates.clear();
    }

    fn take_updates(&mut self) -> Vec<SessionUpdate> {
        std::mem::take(&mut self.pending_updates)
    }

    fn session_cancel(&mut self, session_id: &str) -> Result<()> {
        self.notification(
            "session/cancel",
            serde_json::json!({
                "sessionId": session_id,
            }),
        )
    }

    fn handle_agent_request(
        &mut self,
        id: u64,
        method: &str,
        params: Option<&Value>,
    ) -> Result<()> {
        if method == "session/request_permission" {
            let options = params
                .and_then(|params| params.get("options"))
                .and_then(Value::as_array);
            let selected_option = options.and_then(|options| {
                options
                    .iter()
                    .find(|option| {
                        option.get("kind").and_then(Value::as_str) == Some("reject_once")
                    })
                    .or_else(|| {
                        options.iter().find(|option| {
                            option.get("kind").and_then(Value::as_str) == Some("reject_always")
                        })
                    })
                    .and_then(|option| option.get("optionId").and_then(Value::as_str))
                    .map(ToOwned::to_owned)
            });
            let result = match selected_option {
                Some(option_id) => serde_json::json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id,
                    }
                }),
                None => serde_json::json!({
                    "outcome": {
                        "outcome": "cancelled",
                    }
                }),
            };
            self.write_response(id, Some(result), None)?;
            return Ok(());
        }

        self.write_response(
            id,
            None,
            Some(serde_json::json!({
                "code": -32601,
                "message": format!("Unsupported Hermes ACP client request: {method}"),
            })),
        )?;
        Ok(())
    }

    fn read_until_response(&mut self, expected_id: u64) -> Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self
                .stdout
                .read_line(&mut line)
                .context("reading Hermes ACP stdout")?;
            if bytes == 0 {
                let stderr = self.stderr.lock().expect("stderr buffer lock").clone();
                anyhow::bail!(
                    "Hermes ACP closed stdout before replying to request {expected_id}: {stderr}"
                );
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Value = serde_json::from_str(trimmed)
                .with_context(|| format!("parsing Hermes ACP message: {trimmed}"))?;
            if let Some(method) = msg.get("method").and_then(Value::as_str) {
                if let Some(id) = msg.get("id").and_then(Value::as_u64) {
                    self.handle_agent_request(id, method, msg.get("params"))?;
                } else if method == "session/update" {
                    self.handle_session_update(msg.get("params"))?;
                }
                continue;
            }
            if msg.get("id").and_then(Value::as_u64) == Some(expected_id) {
                if let Some(error) = msg.get("error") {
                    anyhow::bail!("Hermes ACP request {expected_id} failed: {error}");
                }
                return Ok(msg);
            }
        }
    }

    fn handle_session_update(&mut self, params: Option<&Value>) -> Result<()> {
        let Some(params) = params else {
            return Ok(());
        };
        let Some(update) = params.get("update") else {
            return Ok(());
        };
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk") {
            let Some(text) = update
                .get("content")
                .and_then(Value::as_object)
                .and_then(|content| {
                    (content.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| content.get("text").and_then(Value::as_str))
                })
                .flatten()
            else {
                return Ok(());
            };
            self.pending_updates
                .push(SessionUpdate::MessageChunk(text.to_string()));
        }
        Ok(())
    }

    fn write_response(
        &mut self,
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    ) -> Result<()> {
        let response = if let Some(result) = result {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            })
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error.unwrap_or_else(|| serde_json::json!({
                    "code": -32603,
                    "message": "internal error",
                })),
            })
        };
        serde_json::to_writer(&mut self.stdin, &response)
            .context("serializing Hermes ACP response")?;
        self.stdin
            .write_all(b"\n")
            .context("terminating Hermes ACP response line")?;
        self.stdin.flush().context("flushing Hermes ACP response")
    }
}

fn probe_initialize(executable: &Path) -> Result<HermesInitializeProbe> {
    let mut client = AcpClient::spawn(executable)?;
    let result = client.initialize()?;
    client.finish()?;
    Ok(result)
}

fn parse_usage(value: &Value) -> Option<HermesUsage> {
    Some(HermesUsage {
        cached_read_tokens: value.get("cachedReadTokens")?.as_u64()?,
        input_tokens: value.get("inputTokens")?.as_u64()?,
        output_tokens: value.get("outputTokens")?.as_u64()?,
        thought_tokens: value.get("thoughtTokens")?.as_u64()?,
        total_tokens: value.get("totalTokens")?.as_u64()?,
    })
}

fn run_cli_output(executable: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(executable).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn discover_auth_state(executable: &Path) -> Option<HermesAuthState> {
    let providers = run_cli_output(executable, &["auth", "list"])?;
    let provider_ids: Vec<String> = providers
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('(') || trimmed.starts_with('#') {
                return None;
            }
            let (provider, rest) = trimmed.split_once(' ')?;
            rest.contains("credentials").then(|| provider.to_string())
        })
        .collect();

    let mut first_not_logged_in: Option<(String, String)> = None;
    for provider_id in provider_ids {
        let output = run_cli_output(executable, &["auth", "status", &provider_id])?;
        let summary = output.trim().to_string();
        let lower = summary.to_ascii_lowercase();
        if lower.contains("not logged in") || lower.contains("logged out") {
            if first_not_logged_in.is_none() {
                first_not_logged_in = Some((provider_id, summary));
            }
            continue;
        }
        if lower.contains("logged in") {
            return Some(HermesAuthState::LoggedIn {
                provider_id,
                summary,
            });
        }
    }

    first_not_logged_in.map(|(provider_id, summary)| HermesAuthState::NotLoggedIn {
        provider_id: Some(provider_id),
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::contract::run_contract_suite;
    use crate::runner::backends::test_util::make_fake_bin;
    use crate::test_support::ExecGuard;
    use tempfile::TempDir;

    fn fake_hermes_body(version: &str, logged_in: bool) -> String {
        let auth_status = if logged_in {
            "echo 'hermes: logged in'"
        } else {
            "echo 'hermes: logged out'"
        };
        let body = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'Hermes Agent __VERSION__'
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "list" ]; then
  cat <<'EOF'
hermes (1 credentials):
  #1  device_code          oauth   device_code ←
EOF
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
__AUTH_STATUS_LINE__
  exit 0
fi
if [ "$1" = "acp" ]; then
  shift
  PYFILE="$(mktemp "${TMPDIR:-/tmp}/fake-hermes-acp.XXXXXX.py")"
  cat > "$PYFILE" <<'PY'
import json
import sys

def respond(message):
    print(json.dumps(message), flush=True)

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    msg = json.loads(raw)
    if "method" not in msg or "id" not in msg:
        continue
    method = msg["method"]
    request_id = msg["id"]
    params = msg.get("params") or {}
    if method == "initialize":
        respond({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "agentInfo": {"version": "__VERSION__"},
                "agentCapabilities": {
                    "loadSession": True,
                    "sessionCapabilities": {"resume": {}},
                },
            },
        })
    elif method == "session/new":
        respond({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "session-1"}})
    elif method in ("session/load", "session/resume"):
        respond({"jsonrpc": "2.0", "id": request_id, "result": {}})
    elif method == "session/prompt":
        prompt = params.get("prompt") or []
        text = prompt[0].get("text", "") if prompt and prompt[0].get("type") == "text" else ""
        respond({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "ack: " + text},
                }
            },
        })
        respond({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "stopReason": "end_turn",
                "usage": {
                    "cachedReadTokens": 1,
                    "inputTokens": 2,
                    "outputTokens": 3,
                    "thoughtTokens": 4,
                    "totalTokens": 10,
                },
            },
        })
    else:
        respond({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": "unsupported"},
        })
PY
  exec python3 -u "$PYFILE"
  exit 0
fi
echo "unexpected args: $*" >&2
exit 1
"#;
        body.replace("__VERSION__", version)
            .replace("__AUTH_STATUS_LINE__", auth_status)
    }

    fn fake_hermes_executable(logged_in: bool) -> (ExecGuard, TempDir, PathBuf) {
        let exec_guard = ExecGuard::new();
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let hermes = bin_dir.join("hermes");
        make_fake_bin(
            &bin_dir,
            "hermes",
            &fake_hermes_body("9.9.9-test", logged_in),
        );
        (exec_guard, temp, hermes)
    }

    #[test]
    fn discovery_records_version_and_logged_in_auth_state_without_tokens() {
        let (_exec_guard, _temp, hermes) = fake_hermes_executable(true);
        let session = HermesManagerSession::with_executable(hermes.clone());
        let discovery = session.discovery();

        assert_eq!(discovery.executable, hermes);
        assert_eq!(discovery.version.as_deref(), Some("9.9.9-test"));
        match &discovery.auth_state {
            HermesAuthState::LoggedIn {
                provider_id,
                summary,
            } => {
                assert_eq!(provider_id, "hermes");
                assert!(summary.contains("logged in"));
                assert!(!summary.contains("token"));
            }
            other => panic!("unexpected auth state: {other:?}"),
        }
    }

    #[test]
    fn discovery_records_not_logged_in_state() {
        let (_exec_guard, _temp, hermes) = fake_hermes_executable(false);
        let session = HermesManagerSession::with_executable(hermes.clone());
        let discovery = session.discovery();

        match &discovery.auth_state {
            HermesAuthState::NotLoggedIn {
                provider_id,
                summary,
            } => {
                assert_eq!(provider_id.as_deref(), Some("hermes"));
                assert!(summary.contains("logged out"));
            }
            other => panic!("unexpected auth state: {other:?}"),
        }
    }

    #[test]
    fn fake_hermes_adapter_satisfies_the_contract_suite() {
        let (_exec_guard, _temp, hermes) = fake_hermes_executable(true);
        let mut session = HermesManagerSession::with_executable(hermes.clone());
        run_contract_suite(&mut session);
    }

    #[test]
    fn restart_resume_and_usage_are_recorded_from_structured_acp_data() {
        let (_exec_guard, _temp, hermes) = fake_hermes_executable(true);

        let mut first = HermesManagerSession::with_executable(hermes.clone());
        let id = first
            .start(StartRequest {
                profile: "resume-check".to_string(),
                instruction: "Reply with exactly OK.".to_string(),
            })
            .expect("start must succeed");
        let updates = first.stream(&id).expect("stream must succeed");
        assert_eq!(
            updates,
            vec![SessionUpdate::MessageChunk(
                "ack: Reply with exactly OK.".into()
            )]
        );
        assert_eq!(
            first.last_usage(&id).map(|usage| usage.total_tokens),
            Some(10)
        );

        let mut second = HermesManagerSession::with_executable(hermes.clone());
        second
            .resume(&id)
            .expect("resume must succeed across adapter instances");
        second
            .send(&id, "follow up")
            .expect("send must succeed after resume");
        let updates = second
            .stream(&id)
            .expect("stream after resume must succeed");
        assert_eq!(
            updates,
            vec![SessionUpdate::MessageChunk("ack: follow up".into())]
        );
    }
}
