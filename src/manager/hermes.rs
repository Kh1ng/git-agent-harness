//! Hermes-backed `ManagerSession` adapter.
//!
//! Hermes exposes the ACP server on stdio (`hermes acp`). That gives us a
//! structured session lifecycle with real session IDs, prompt turns, prompt
//! cancellation, and replay/resume support without scraping presentation
//! prose. A reader thread owns ACP stdout so prompt requests do not block
//! `start` or `send`; the public adapter remains synchronous and object-safe.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

const ACP_PROTOCOL_VERSION: u64 = 1;
const DEFAULT_HERMES_PROFILE: &str = "gah-manager";

#[derive(Serialize, Deserialize)]
struct DurableSessionMapping {
    gah_session_id: String,
    provider_session_id: String,
}

fn resolve_session_dir(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf> {
    if let Some(dir) = xdg_state_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        return Ok(dir.join("gah").join("manager-sessions").join("hermes"));
    }
    if let Some(home) = home.map(Path::new).filter(|path| path.is_absolute()) {
        return Ok(home.join(".local/state/gah/manager-sessions/hermes"));
    }
    Err(anyhow!(
        "Hermes session persistence requires an absolute XDG_STATE_HOME or HOME"
    ))
}

fn default_session_dir() -> Result<PathBuf> {
    resolve_session_dir(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn mapping_path(session_dir: &Path, session: &GahSessionId) -> PathBuf {
    let digest = Sha256::digest(session.as_str().as_bytes());
    session_dir.join(format!("{digest:x}.json"))
}

fn persist_mapping(
    session_dir: &Path,
    session: &GahSessionId,
    provider_session_id: &str,
) -> Result<()> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating Hermes session map {}", session_dir.display()))?;
    let path = mapping_path(session_dir, session);
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let mapping = DurableSessionMapping {
        gah_session_id: session.as_str().to_string(),
        provider_session_id: provider_session_id.to_string(),
    };
    let mut file = File::create(&temp)
        .with_context(|| format!("creating Hermes session map {}", temp.display()))?;
    serde_json::to_writer(&mut file, &mapping).context("serializing Hermes session map")?;
    file.sync_all().ok();
    fs::rename(&temp, &path)
        .with_context(|| format!("committing Hermes session map {}", path.display()))?;
    Ok(())
}

fn load_mapping(session_dir: &Path, session: &GahSessionId) -> Result<String> {
    let path = mapping_path(session_dir, session);
    let file = File::open(&path)
        .with_context(|| format!("opening Hermes session map {}", path.display()))?;
    let mapping: DurableSessionMapping = serde_json::from_reader(file)
        .with_context(|| format!("parsing Hermes session map {}", path.display()))?;
    if mapping.gah_session_id != session.as_str() {
        return Err(anyhow!(
            "Hermes session map identity mismatch for {session}"
        ));
    }
    Ok(mapping.provider_session_id)
}

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
    messages: Receiver<std::result::Result<Value, String>>,
    next_id: u64,
    supports_load_session: bool,
    supports_resume: bool,
    #[cfg(test)]
    fail_request: Option<&'static str>,
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
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || read_messages(stdout, sender));
        Ok(Self {
            child,
            stdin,
            messages,
            next_id: 1,
            supports_load_session: false,
            supports_resume: false,
            #[cfg(test)]
            fail_request: None,
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

    fn send_request(&mut self, method: &str, params: Value) -> Result<u64> {
        #[cfg(test)]
        if self.fail_request == Some(method) {
            self.fail_request = None;
            return Err(anyhow!("injected {method} enqueue failure"));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        Ok(id)
    }

    fn recv(&self) -> Result<Value> {
        match self.messages.recv() {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(_) => Err(anyhow!("Hermes ACP process closed its output")),
        }
    }

    fn try_recv(&self) -> Result<Option<Value>> {
        match self.messages.try_recv() {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(anyhow!("Hermes ACP process closed its output")),
        }
    }

    fn respond_to_server_request(&mut self, message: &Value) -> Result<bool> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(false);
        };
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(false);
        };
        if method == "session/request_permission" {
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
                .map(|option_id| json!({"outcome": {"outcome": "selected", "optionId": option_id}}))
                .unwrap_or_else(|| json!({"outcome": {"outcome": "cancelled"}}));
            self.write_json(&json!({"jsonrpc": "2.0", "id": id, "result": result}))?;
        } else {
            self.write_json(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("unsupported client request {method}")}
            }))?;
        }
        Ok(true)
    }
}

fn read_messages(stdout: ChildStdout, sender: mpsc::Sender<std::result::Result<Value, String>>) {
    for line in BufReader::new(stdout).lines() {
        let message = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => serde_json::from_str(line.trim())
                .map_err(|error| format!("parsing Hermes ACP line: {error}")),
            Err(error) => Err(format!("reading Hermes ACP output: {error}")),
        };
        if sender.send(message).is_err() {
            return;
        }
    }
}

fn rpc_request(transport: &mut HermesTransport, method: &str, params: Value) -> Result<Value> {
    let id = transport.send_request(method, params)?;

    loop {
        let message = transport.recv()?;

        if is_response_for(&message, id) {
            return decode_json_rpc_response(message);
        }
        transport.respond_to_server_request(&message)?;
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
            .get("data")
            .and_then(|data| data.get("message"))
            .and_then(Value::as_str)
            .or_else(|| error.get("message").and_then(Value::as_str))
            .unwrap_or("Hermes ACP request failed");
        return Err(anyhow!("{message} (code {code})"));
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

fn extract_updates(update: &Value) -> Vec<SessionUpdate> {
    let Some(kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
        return vec![];
    };
    match kind {
        "agent_message_chunk" => extract_texts(update)
            .into_iter()
            .map(SessionUpdate::MessageChunk)
            .collect(),
        "usage_update" => match (
            update.get("used").and_then(Value::as_u64),
            update.get("size").and_then(Value::as_u64),
        ) {
            (Some(used), Some(size)) => vec![SessionUpdate::Usage { used, size }],
            _ => vec![],
        },
        _ => vec![],
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
        return HermesAuthState::Error(format!("status command exited with {}", output.status));
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lower = combined.to_lowercase();
    let logged_in = lower.lines().any(|line| {
        (line.contains("logged in") && !line.contains("not logged in"))
            || (line.contains("authenticated")
                && !line.contains("not authenticated")
                && !line.contains("unauthenticated"))
    });
    if logged_in {
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
        Err(_) => HermesAuthState::Error("status command could not be started".to_string()),
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
    prompt_requests: HashMap<u64, GahSessionId>,
    capabilities: SessionCapabilities,
    session_dir: PathBuf,
}

impl HermesManagerSession {
    pub fn discover(executable: impl AsRef<Path>) -> Result<HermesDiscovery> {
        discover(executable)
    }

    pub fn new(executable: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_session_dir(executable, default_session_dir()?)
    }

    fn new_with_session_dir(
        executable: impl AsRef<Path>,
        session_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        let discovery = discover(executable.as_ref())?;
        let mut transport = HermesTransport::spawn(&discovery.executable)?;
        let response = rpc_request(
            &mut transport,
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
            .is_some_and(|value| value != &Value::Bool(false));
        let capabilities = SessionCapabilities {
            resume: transport.supports_load_session || transport.supports_resume,
            interrupt: true,
            inspect: false,
        };
        Ok(Self {
            discovery,
            transport,
            sessions: HashMap::new(),
            prompt_requests: HashMap::new(),
            capabilities,
            session_dir: session_dir.into(),
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

    fn provider_session_id(&self, session: &GahSessionId) -> Result<String> {
        if let Some(state) = self.sessions.get(session) {
            return Ok(state.provider_session_id.clone());
        }
        load_mapping(&self.session_dir, session)
    }

    fn record_session(
        &mut self,
        session: GahSessionId,
        provider_session_id: String,
        status: SessionStatus,
    ) {
        self.sessions.insert(
            session,
            HermesSessionState {
                provider_session_id,
                pending_updates: Vec::new(),
                status,
            },
        );
    }

    fn discard_started_session(&mut self, session: &GahSessionId, provider_session_id: &str) {
        let _ = self.transport.cancel(provider_session_id);
        self.sessions.remove(session);
        self.prompt_requests.retain(|_, id| id != session);
    }

    fn handle_message(&mut self, message: Value) -> Result<()> {
        if self.transport.respond_to_server_request(&message)? {
            return Ok(());
        }
        if message.get("method").and_then(Value::as_str) == Some("session/update") {
            let Some(params) = message.get("params") else {
                return Ok(());
            };
            let Some(provider_id) = params.get("sessionId").and_then(Value::as_str) else {
                return Ok(());
            };
            let updates = params
                .get("update")
                .map(extract_updates)
                .unwrap_or_default();
            if let Some(state) = self
                .sessions
                .values_mut()
                .find(|state| state.provider_session_id == provider_id)
            {
                state.pending_updates.extend(updates);
            }
            return Ok(());
        }
        let Some(request_id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(());
        };
        let Some(session_id) = self.prompt_requests.remove(&request_id) else {
            return Ok(());
        };
        let terminal = match decode_json_rpc_response(message) {
            Ok(_) => TerminalStatus::Completed,
            Err(error) => TerminalStatus::Failed(error.to_string()),
        };
        let still_working = self.prompt_requests.values().any(|id| id == &session_id);
        if let Some(state) = self.sessions.get_mut(&session_id) {
            match (&state.status, terminal, still_working) {
                (SessionStatus::Terminated(TerminalStatus::Interrupted), _, _) => {}
                (SessionStatus::Terminated(TerminalStatus::Failed(_)), _, _) => {}
                (_, TerminalStatus::Failed(error), _) => {
                    state.status = SessionStatus::Terminated(TerminalStatus::Failed(error));
                }
                (_, TerminalStatus::Completed, false) => {
                    state.status = SessionStatus::Terminated(TerminalStatus::Completed);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        while let Some(message) = self.transport.try_recv()? {
            self.handle_message(message)?;
        }
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let request_id = self.transport.send_request(method, params)?;
        loop {
            let message = self.transport.recv()?;
            if is_response_for(&message, request_id) {
                return decode_json_rpc_response(message);
            }
            self.handle_message(message)?;
        }
    }

    fn prompt(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let provider_session_id = self
            .sessions
            .get(session)
            .map(|state| state.provider_session_id.clone())
            .ok_or_else(|| anyhow!("Hermes session {session} must be resumed before sending"))?;
        let request_id = self.transport.send_request(
            "session/prompt",
            json!({
                "sessionId": provider_session_id,
                "prompt": [{"type": "text", "text": message}]
            }),
        )?;
        self.prompt_requests.insert(request_id, session.clone());
        self.state_mut(session)?.status = SessionStatus::Working;
        Ok(())
    }
}

impl ManagerSession for HermesManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let cwd = std::env::current_dir().context("resolving current directory for Hermes ACP")?;
        let response = self.request(
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
        let gah_session_id = GahSessionId::new(&request.profile);
        self.record_session(
            gah_session_id.clone(),
            provider_session_id.clone(),
            SessionStatus::Idle,
        );
        if let Err(error) = self.prompt(&gah_session_id, &request.instruction) {
            self.discard_started_session(&gah_session_id, &provider_session_id);
            return Err(error)
                .with_context(|| format!("prompting Hermes session {provider_session_id}"));
        }
        if let Err(error) =
            persist_mapping(&self.session_dir, &gah_session_id, &provider_session_id)
        {
            self.discard_started_session(&gah_session_id, &provider_session_id);
            return Err(error);
        }
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
        let provider_session_id = self.provider_session_id(session)?;
        let restored = !self.sessions.contains_key(session);
        if restored {
            self.record_session(
                session.clone(),
                provider_session_id.clone(),
                SessionStatus::Idle,
            );
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
        let result = self
            .request(
                method,
                json!({
                    "sessionId": provider_session_id,
                    "cwd": cwd,
                    "mcpServers": [],
                }),
            )
            .with_context(|| format!("resuming Hermes session {provider_session_id}"));
        if let Err(error) = result {
            if restored {
                self.sessions.remove(session);
            }
            return Err(error);
        }
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Idle;
        }
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        self.prompt(session, message)
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        self.pump()?;
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
        let provider_session_id = self
            .sessions
            .get(session)
            .map(|state| state.provider_session_id.clone())
            .ok_or_else(|| {
                anyhow!("Hermes session {session} must be resumed before interrupting")
            })?;
        self.transport.cancel(&provider_session_id)?;
        self.prompt_requests.retain(|_, id| id != session);
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
        self.pump()?;
        let state = self.state_mut(session)?;
        Ok(match &state.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}

impl Drop for HermesManagerSession {
    fn drop(&mut self) {
        let _ = crate::runner::process::kill_process_group(&mut self.transport.child);
        let _ = self.transport.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::{fixture, make_fake_bin};
    use std::time::{Duration, Instant};

    fn wait_for_updates(
        session: &mut HermesManagerSession,
        id: &GahSessionId,
    ) -> Vec<SessionUpdate> {
        for _ in 0..100 {
            let updates = session.stream(id).unwrap();
            if !updates.is_empty() {
                return updates;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for Hermes updates");
    }

    fn wait_for_terminal(session: &mut HermesManagerSession, id: &GahSessionId) -> TerminalStatus {
        for _ in 0..100 {
            if let Some(status) = session.terminal_status(id).unwrap() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for Hermes terminal status");
    }

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
import time

record = os.environ["RECORD_PATH"]
session_counter = 0
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
            session_counter += 1
            resp = {{
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {{
                    "sessionId": "sess-" + str(session_counter)
                }}
            }}
            print(json.dumps(resp), flush=True)
        elif method in ("session/resume", "session/load"):
            if msg["params"]["sessionId"] == "sess-fail":
                print(json.dumps({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32000, "message": "resume failed"}}}}), flush=True)
            else:
                print(json.dumps({{"jsonrpc": "2.0", "id": req_id, "result": {{}} }}), flush=True)
        elif method == "session/prompt":
            session_id = msg["params"]["sessionId"]
            prompt = msg["params"]["prompt"][0]["text"]
            if prompt == "fail":
                print(json.dumps({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32001, "message": "prompt failed"}}}}), flush=True)
                continue
            update = {{
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {{
                    "sessionId": session_id,
                    "update": {{
                        "sessionUpdate": "agent_message_chunk",
                        "content": {{"type": "text", "text": "reply: " + prompt}}
                    }}
                }}
            }}
            print(json.dumps(update), flush=True)
            print(json.dumps({{
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {{
                    "sessionId": session_id,
                    "update": {{"sessionUpdate": "usage_update", "used": 12, "size": 4096}}
                }}
            }}), flush=True)
            if prompt == "slow":
                time.sleep(1)
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
        let _exec_guard = crate::test_support::ExecGuard::new();
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
    fn discovery_does_not_retain_failed_status_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "hermes",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\necho 'Authorization: Bearer sk-secret' >&2\nexit 1\n",
        );

        let discovery = discover(f.bin_dir.join("hermes")).unwrap();
        let HermesAuthState::Error(message) = discovery.auth_state else {
            panic!("expected failed status discovery");
        };
        assert!(!message.contains("sk-secret"));
    }

    #[test]
    fn session_directory_requires_absolute_user_state() {
        assert_eq!(
            resolve_session_dir(Some(OsStr::new("/state")), Some(OsStr::new("/home/user")))
                .unwrap(),
            PathBuf::from("/state/gah/manager-sessions/hermes")
        );
        assert_eq!(
            resolve_session_dir(Some(OsStr::new("")), Some(OsStr::new("/home/user"))).unwrap(),
            PathBuf::from("/home/user/.local/state/gah/manager-sessions/hermes")
        );
        assert!(resolve_session_dir(Some(OsStr::new("relative")), None).is_err());
    }

    #[test]
    fn structured_updates_exclude_user_echoes_and_thoughts() {
        assert!(extract_updates(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"text": "secret user prompt"}
        }))
        .is_empty());
        assert!(extract_updates(&json!({
            "sessionUpdate": "thought_chunk",
            "content": {"text": "hidden reasoning"}
        }))
        .is_empty());
        assert_eq!(
            extract_updates(&json!({
                "sessionUpdate": "usage_update",
                "used": 12,
                "size": 4096
            })),
            vec![SessionUpdate::Usage {
                used: 12,
                size: 4096
            }]
        );
    }

    #[test]
    fn rpc_errors_prefer_structured_detail() {
        let error = decode_json_rpc_response(json!({
            "error": {
                "code": -32000,
                "message": "request failed",
                "data": {"message": "session not found"}
            }
        }))
        .unwrap_err();
        assert_eq!(error.to_string(), "session not found (code -32000)");
    }

    #[test]
    fn adapter_runs_prompt_and_streams_message_chunks() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);

        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        let updates = wait_for_updates(&mut session, &id);
        assert_eq!(
            updates,
            vec![
                SessionUpdate::MessageChunk("reply: hello".into()),
                SessionUpdate::Usage {
                    used: 12,
                    size: 4096
                }
            ]
        );
        assert_eq!(
            wait_for_terminal(&mut session, &id),
            TerminalStatus::Completed
        );
    }

    #[test]
    fn prompt_returns_before_completion_and_can_be_interrupted() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();

        let started = Instant::now();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        session.interrupt(&id).unwrap();
        assert_eq!(
            session.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Interrupted)
        );
    }

    #[test]
    fn prompt_failure_becomes_terminal_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "fail".into(),
            })
            .unwrap();

        let TerminalStatus::Failed(message) = wait_for_terminal(&mut session, &id) else {
            panic!("expected failed prompt status");
        };
        assert!(message.contains("prompt failed"));
    }

    #[test]
    fn failed_mapping_commit_removes_the_started_session() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("not-a-directory");
        fs::write(&session_dir, "occupied").unwrap();
        let mut session =
            HermesManagerSession::new_with_session_dir(f.bin_dir.join("hermes"), &session_dir)
                .unwrap();

        assert!(session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .is_err());
        assert!(session.sessions.is_empty());
        assert!(session.prompt_requests.is_empty());
    }

    #[test]
    fn failed_initial_prompt_cancels_the_provider_session() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        session.transport.fail_request = Some("session/prompt");

        assert!(session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .is_err());
        assert!(session.sessions.is_empty());
        assert!(session.prompt_requests.is_empty());
        let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
        assert!(requests.contains("session/cancel"));
    }

    #[test]
    fn adapter_resume_and_interrupt_round_trip() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);

        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
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
    fn adapter_passes_shared_contract_suite() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let mut session = HermesManagerSession::new_with_session_dir(
            f.bin_dir.join("hermes"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        crate::manager::contract::run_contract_suite(&mut session);
    }

    #[test]
    fn restart_resumes_through_durable_gah_to_hermes_mapping() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let id = {
            let mut adapter =
                HermesManagerSession::new_with_session_dir(f.bin_dir.join("hermes"), &session_dir)
                    .unwrap();
            adapter
                .start(StartRequest {
                    profile: "profile-a".into(),
                    instruction: "hello".into(),
                })
                .unwrap()
        };
        assert!(!id.as_str().contains("sess-"));

        let restored_id = id.as_str().parse::<GahSessionId>().unwrap();
        let mut restarted =
            HermesManagerSession::new_with_session_dir(f.bin_dir.join("hermes"), &session_dir)
                .unwrap();
        restarted.resume(&restored_id).unwrap();
        restarted.send(&restored_id, "after restart").unwrap();
        assert!(wait_for_updates(&mut restarted, &restored_id)
            .contains(&SessionUpdate::MessageChunk("reply: after restart".into())));
    }

    #[test]
    fn failed_restart_resume_does_not_unlock_send() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_hermes(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let id = GahSessionId::new("profile-a");
        persist_mapping(&session_dir, &id, "sess-fail").unwrap();
        let mut restarted =
            HermesManagerSession::new_with_session_dir(f.bin_dir.join("hermes"), &session_dir)
                .unwrap();

        assert!(restarted.resume(&id).is_err());
        assert!(restarted
            .send(&id, "must not bypass resume")
            .unwrap_err()
            .to_string()
            .contains("must be resumed"));
    }
}
