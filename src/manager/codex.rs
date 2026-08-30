//! Codex-backed `ManagerSession` adapter (issue #817, split from #520).
//!
//! Codex's `codex app-server` speaks stdio JSON-RPC, one NDJSON object per
//! line -- same shape as Hermes's ACP transport in `manager::hermes`, just a
//! different method/notification vocabulary. This is *not*
//! `runner::backends::codex`'s bounded `codex exec --json` path; it is a
//! long-lived process with a real session lifecycle (`thread/start`,
//! `thread/resume`, `turn/start`, `turn/interrupt`) and structured
//! notifications instead of scraped presentation prose.
//!
//! The exact methods/params/notifications this module speaks were verified
//! against the installed `codex-cli 0.145.0` binary before writing this
//! adapter: `codex app-server generate-json-schema --out DIR` (no
//! `--experimental` flag) already includes `thread/start`, `thread/resume`,
//! `turn/start`, `turn/steer`, `turn/interrupt`, `item/agentMessage/delta`,
//! `thread/tokenUsage/updated`, and `turn/completed` in the *stable*
//! schema bundle, and a live handshake (`initialize` -> `thread/start` ->
//! `turn/start` -> `turn/steer`/`turn/interrupt` -> restart ->
//! `thread/resume` -> `turn/start`) round-tripped exactly as documented,
//! including a real `turn/steer` while a turn was genuinely active (it
//! returns the same `turnId`, and the turn keeps streaming under it).
//! `detect_stable_methods` re-runs that same schema command at construction
//! time so `resume`/`interrupt` capability detection tracks the actual
//! installed binary instead of a hardcoded assumption -- an older
//! app-server missing one of these methods gets an honest `false` instead
//! of a generic RPC error the first time it's called.
//! `installed_codex_passes_handshake_smoke_when_requested` and
//! `installed_codex_adapter_completes_a_real_turn_and_resumes_after_restart_when_requested`
//! below re-verify this against whatever `codex` is on `PATH` at test time,
//! gated behind an env var so they don't run (or burn API budget) on every
//! CI run of a real Codex installation. Issue #527 (the "versioned
//! app-server transport and generated schemas" ticket this issue's scope
//! note points at) was closed without a merged implementation, so there is
//! no generated-bindings package to build on; this adapter talks the wire
//! protocol directly instead.

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

const CLIENT_NAME: &str = "git-agent-harness";

#[derive(Serialize, Deserialize)]
struct DurableThreadMapping {
    gah_session_id: String,
    thread_id: String,
}

fn resolve_session_dir(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf> {
    if let Some(dir) = xdg_state_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        return Ok(dir.join("gah").join("manager-sessions").join("codex"));
    }
    if let Some(home) = home.map(Path::new).filter(|path| path.is_absolute()) {
        return Ok(home.join(".local/state/gah/manager-sessions/codex"));
    }
    Err(anyhow!(
        "Codex session persistence requires an absolute XDG_STATE_HOME or HOME"
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

fn persist_mapping(session_dir: &Path, session: &GahSessionId, thread_id: &str) -> Result<()> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating Codex session map {}", session_dir.display()))?;
    let path = mapping_path(session_dir, session);
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let mapping = DurableThreadMapping {
        gah_session_id: session.as_str().to_string(),
        thread_id: thread_id.to_string(),
    };
    let mut file = File::create(&temp)
        .with_context(|| format!("creating Codex session map {}", temp.display()))?;
    serde_json::to_writer(&mut file, &mapping).context("serializing Codex session map")?;
    file.sync_all().ok();
    fs::rename(&temp, &path)
        .with_context(|| format!("committing Codex session map {}", path.display()))?;
    Ok(())
}

fn load_mapping(session_dir: &Path, session: &GahSessionId) -> Result<String> {
    let path = mapping_path(session_dir, session);
    let file = File::open(&path)
        .with_context(|| format!("opening Codex session map {}", path.display()))?;
    let mapping: DurableThreadMapping = serde_json::from_reader(file)
        .with_context(|| format!("parsing Codex session map {}", path.display()))?;
    if mapping.gah_session_id != session.as_str() {
        return Err(anyhow!("Codex session map identity mismatch for {session}"));
    }
    Ok(mapping.thread_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexAuthState {
    Unknown,
    LoggedOut,
    LoggedIn,
    Error(String),
}

impl fmt::Display for CodexAuthState {
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
pub struct CodexDiscovery {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub auth_state: CodexAuthState,
}

#[derive(Debug)]
struct CodexSessionState {
    thread_id: String,
    pending_updates: Vec<SessionUpdate>,
    status: SessionStatus,
    active_turn_id: Option<String>,
}

#[derive(Debug)]
struct CodexTransport {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<std::result::Result<Value, String>>,
    next_id: u64,
    #[cfg(test)]
    fail_request: Option<&'static str>,
}

impl CodexTransport {
    fn spawn(executable: &Path) -> Result<Self> {
        let mut cmd = Command::new(executable);
        cmd.arg("app-server");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        crate::runner::process::prepare_process_group(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("launching Codex app-server from {}", executable.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Codex app-server child did not provide stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Codex app-server child did not provide stdout"))?;
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || read_messages(stdout, sender));
        Ok(Self {
            child,
            stdin,
            messages,
            next_id: 1,
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
            Err(_) => Err(anyhow!("Codex app-server process closed its output")),
        }
    }

    fn try_recv(&self) -> Result<Option<Value>> {
        match self.messages.try_recv() {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(anyhow!("Codex app-server process closed its output"))
            }
        }
    }

    /// Any inbound message with both `method` and `id` is a server->client
    /// request (e.g. an approval ask). We always start threads with
    /// `approvalPolicy: "never"` so none of these are expected in normal
    /// operation, but an unattended manager loop must never hang waiting to
    /// answer one it doesn't recognize -- decline it instead.
    fn respond_to_server_request(&mut self, message: &Value) -> Result<bool> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(false);
        };
        let Some(id) = message.get("id") else {
            return Ok(false);
        };
        self.write_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("git-agent-harness's Codex manager adapter does not support server request '{method}'"),
            }
        }))?;
        Ok(true)
    }
}

fn read_messages(stdout: ChildStdout, sender: mpsc::Sender<std::result::Result<Value, String>>) {
    for line in BufReader::new(stdout).lines() {
        let message = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => serde_json::from_str(line.trim())
                .map_err(|error| format!("parsing Codex app-server line: {error}")),
            Err(error) => Err(format!("reading Codex app-server output: {error}")),
        };
        if sender.send(message).is_err() {
            return;
        }
    }
}

fn rpc_request(transport: &mut CodexTransport, method: &str, params: Value) -> Result<Value> {
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
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex app-server request failed");
        return Err(anyhow!("{message} (code {code})"));
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

fn parse_version_from_output(output: &std::process::Output) -> Option<String> {
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn classify_auth_state(output: &std::process::Output) -> CodexAuthState {
    if !output.status.success() {
        return CodexAuthState::Error(format!(
            "login status command exited with {}",
            output.status
        ));
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lower = combined.to_lowercase();
    let logged_in = lower
        .lines()
        .any(|line| line.contains("logged in") && !line.contains("not logged in"));
    if logged_in {
        CodexAuthState::LoggedIn
    } else if lower.contains("not logged in")
        || lower.contains("logged out")
        || lower.contains("no credentials")
    {
        CodexAuthState::LoggedOut
    } else {
        CodexAuthState::Unknown
    }
}

fn discover(executable: impl AsRef<Path>) -> Result<CodexDiscovery> {
    let executable = executable.as_ref().to_path_buf();
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output))
        .and_then(|output| parse_version_from_output(&output));

    let auth_state = match Command::new(&executable).args(["login", "status"]).output() {
        Ok(output) => classify_auth_state(&output),
        Err(_) => CodexAuthState::Error("login status command could not be started".to_string()),
    };

    Ok(CodexDiscovery {
        executable,
        version,
        auth_state,
    })
}

/// Real provider signal for capability detection: `codex app-server
/// generate-json-schema` (no `--experimental`) emits the installed binary's
/// actual *stable* method set as `ClientRequest.json`'s `oneOf[].method`
/// enum -- this is the same command used to verify this module's protocol
/// against 0.145.0 (see module doc comment), so reusing it at construction
/// time means capability detection tracks whatever binary is actually
/// installed instead of a hardcoded assumption. Empty set (including "the
/// binary is old enough this subcommand doesn't exist") fails closed --
/// every dependent capability comes back false rather than guessed true.
fn detect_stable_methods(executable: &Path) -> std::collections::HashSet<String> {
    let dir = std::env::temp_dir().join(format!(
        "gah-codex-schema-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let methods = (|| -> Result<std::collections::HashSet<String>> {
        fs::create_dir_all(&dir)?;
        let output = Command::new(executable)
            .args(["app-server", "generate-json-schema", "--out"])
            .arg(&dir)
            .output()
            .context("running codex app-server generate-json-schema")?;
        if !output.status.success() {
            anyhow::bail!(
                "codex app-server generate-json-schema exited with {}",
                output.status
            );
        }
        let contents = fs::read_to_string(dir.join("ClientRequest.json"))
            .context("reading generated ClientRequest.json")?;
        let schema: Value = serde_json::from_str(&contents)?;
        Ok(schema
            .get("oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("properties")?
                    .get("method")?
                    .get("enum")?
                    .get(0)?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect())
    })();
    let _ = fs::remove_dir_all(&dir);
    methods.unwrap_or_default()
}

fn is_no_active_turn_error(error: &anyhow::Error) -> bool {
    error.to_string().contains("no active turn")
}

pub struct CodexManagerSession {
    discovery: CodexDiscovery,
    transport: CodexTransport,
    sessions: HashMap<GahSessionId, CodexSessionState>,
    capabilities: SessionCapabilities,
    session_dir: PathBuf,
}

impl CodexManagerSession {
    pub fn discover(executable: impl AsRef<Path>) -> Result<CodexDiscovery> {
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
        let mut transport = CodexTransport::spawn(&discovery.executable)?;
        rpc_request(
            &mut transport,
            "initialize",
            json!({
                "clientInfo": {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        transport.send_notification("initialized", json!({}))?;
        // thread/resume and turn/interrupt are detected from the installed
        // binary's own generated schema (see `detect_stable_methods`)
        // instead of assumed -- an older/incompatible app-server that
        // doesn't advertise one of these methods gets an honest `false`
        // here instead of a generic RPC error the first time it's called.
        // `inspect` never touches the wire (it only reads locally-tracked
        // state updated by `pump`), so it has no real-provider dependency
        // and is always supported.
        let methods = detect_stable_methods(&discovery.executable);
        let capabilities = SessionCapabilities {
            resume: methods.contains("thread/resume"),
            interrupt: methods.contains("turn/interrupt"),
            inspect: true,
        };
        Ok(Self {
            discovery,
            transport,
            sessions: HashMap::new(),
            capabilities,
            session_dir: session_dir.into(),
        })
    }

    pub fn discovery(&self) -> &CodexDiscovery {
        &self.discovery
    }

    fn state_mut(&mut self, session: &GahSessionId) -> Result<&mut CodexSessionState> {
        self.sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("unknown Codex session {session}"))
    }

    fn session_by_thread_mut(&mut self, thread_id: &str) -> Option<&mut CodexSessionState> {
        self.sessions
            .values_mut()
            .find(|state| state.thread_id == thread_id)
    }

    fn provider_thread_id(&self, session: &GahSessionId) -> Result<String> {
        if let Some(state) = self.sessions.get(session) {
            return Ok(state.thread_id.clone());
        }
        load_mapping(&self.session_dir, session)
    }

    fn handle_message(&mut self, message: Value) -> Result<()> {
        if self.transport.respond_to_server_request(&message)? {
            return Ok(());
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A stray response with no in-flight `request()` correlating it
            // (e.g. a late race) -- nothing to route it to.
            return Ok(());
        };
        let Some(params) = message.get("params") else {
            return Ok(());
        };
        match method {
            "item/agentMessage/delta" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let Some(delta) = params.get("delta").and_then(Value::as_str) else {
                    return Ok(());
                };
                if let Some(state) = self.session_by_thread_mut(thread_id) {
                    state
                        .pending_updates
                        .push(SessionUpdate::MessageChunk(delta.to_owned()));
                }
            }
            "thread/tokenUsage/updated" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let Some(used) = params
                    .get("tokenUsage")
                    .and_then(|usage| usage.get("total"))
                    .and_then(|total| total.get("totalTokens"))
                    .and_then(Value::as_u64)
                else {
                    return Ok(());
                };
                let size = params
                    .get("tokenUsage")
                    .and_then(|usage| usage.get("modelContextWindow"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if let Some(state) = self.session_by_thread_mut(thread_id) {
                    state
                        .pending_updates
                        .push(SessionUpdate::Usage { used, size });
                }
            }
            "turn/started" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let turn_id = params
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(state) = self.session_by_thread_mut(thread_id) {
                    state.status = SessionStatus::Working;
                    if turn_id.is_some() {
                        state.active_turn_id = turn_id;
                    }
                }
            }
            "turn/completed" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let Some(turn) = params.get("turn") else {
                    return Ok(());
                };
                let terminal = match turn.get("status").and_then(Value::as_str) {
                    Some("completed") => Some(TerminalStatus::Completed),
                    Some("interrupted") => Some(TerminalStatus::Interrupted),
                    Some("failed") => {
                        let message = turn
                            .get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("Codex turn failed")
                            .to_owned();
                        Some(TerminalStatus::Failed(message))
                    }
                    _ => None,
                };
                if let Some(terminal) = terminal {
                    if let Some(state) = self.session_by_thread_mut(thread_id) {
                        state.status = SessionStatus::Terminated(terminal);
                        state.active_turn_id = None;
                    }
                }
            }
            "error" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                if params
                    .get("willRetry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return Ok(());
                }
                let message = params
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported an error")
                    .to_owned();
                if let Some(state) = self.session_by_thread_mut(thread_id) {
                    state.status = SessionStatus::Terminated(TerminalStatus::Failed(message));
                    state.active_turn_id = None;
                }
            }
            _ => {}
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

    fn start_turn(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let thread_id = self
            .sessions
            .get(session)
            .map(|state| state.thread_id.clone())
            .ok_or_else(|| anyhow!("Codex session {session} must be resumed before sending"))?;
        let response = self.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": message}],
            }),
        )?;
        let turn_id = response
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Working;
            state.active_turn_id = turn_id;
        }
        Ok(())
    }

    /// Steers the thread's currently in-progress turn instead of starting a
    /// new one. `turn/steer` is the stable app-server method for this (see
    /// module doc comment); it requires `expectedTurnId` as a precondition
    /// and errors with "no active turn to steer" if the turn has already
    /// ended, which `send` treats as a race and falls back on.
    fn steer_turn(&mut self, session: &GahSessionId, turn_id: &str, message: &str) -> Result<()> {
        let thread_id = self.state_mut(session)?.thread_id.clone();
        self.request(
            "turn/steer",
            json!({
                "threadId": thread_id,
                "expectedTurnId": turn_id,
                "input": [{"type": "text", "text": message}],
            }),
        )?;
        Ok(())
    }

    /// Best-effort cleanup for a thread/turn GAH has already started
    /// remotely but cannot keep local state for (mirrors
    /// `manager::hermes::HermesManagerSession::discard_started_session`).
    /// The interrupt result is ignored the same way Hermes ignores its
    /// `session/cancel` result: this is cleanup, not the operation the
    /// caller asked for, so a failure here must not shadow the real error
    /// that triggered the rollback.
    fn discard_started_session(
        &mut self,
        session: &GahSessionId,
        thread_id: &str,
        turn_id: Option<&str>,
    ) {
        if let Some(turn_id) = turn_id {
            let _ = self.request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
            );
        }
        self.sessions.remove(session);
    }
}

impl ManagerSession for CodexManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let cwd =
            std::env::current_dir().context("resolving current directory for Codex app-server")?;
        let response = self.request(
            "thread/start",
            json!({
                "cwd": cwd,
                "approvalPolicy": "never",
            }),
        )?;
        let thread_id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Codex thread/start response did not include a thread id"))?
            .to_owned();
        let gah_session_id = GahSessionId::new(&request.profile);
        self.sessions.insert(
            gah_session_id.clone(),
            CodexSessionState {
                thread_id: thread_id.clone(),
                pending_updates: Vec::new(),
                status: SessionStatus::Idle,
                active_turn_id: None,
            },
        );
        if let Err(error) = self.start_turn(&gah_session_id, &request.instruction) {
            // ponytail: turn/start itself failed, so no turn is running on
            // Codex's side -- nothing to interrupt, just drop the thread
            // locally. It sits idle on Codex's side at no cost until
            // Codex's own retention/GC reclaims it. Add explicit
            // `thread/delete` here if that ever proves to matter.
            self.sessions.remove(&gah_session_id);
            return Err(error)
                .with_context(|| format!("starting Codex turn on thread {thread_id}"));
        }
        if let Err(error) = persist_mapping(&self.session_dir, &gah_session_id, &thread_id) {
            // The turn *did* start successfully this time, so it's actually
            // running remotely -- unlike the turn/start failure above,
            // dropping local state here would abandon a live turn. Ask
            // Codex to stop it before discarding our only handle to it.
            let turn_id = self
                .sessions
                .get(&gah_session_id)
                .and_then(|state| state.active_turn_id.clone());
            self.discard_started_session(&gah_session_id, &thread_id, turn_id.as_deref());
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
        let thread_id = self.provider_thread_id(session)?;
        let restored = !self.sessions.contains_key(session);
        if restored {
            self.sessions.insert(
                session.clone(),
                CodexSessionState {
                    thread_id: thread_id.clone(),
                    pending_updates: Vec::new(),
                    status: SessionStatus::Idle,
                    active_turn_id: None,
                },
            );
        }
        let result = self
            .request("thread/resume", json!({ "threadId": thread_id }))
            .with_context(|| format!("resuming Codex thread {thread_id}"));
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
        // Drain any pending turn/completed notification first so a turn
        // that just finished (but whose notification hasn't been pumped
        // yet) doesn't look active below.
        self.pump()?;
        let active_turn_id = self
            .sessions
            .get(session)
            .and_then(|state| state.active_turn_id.clone());
        if let Some(turn_id) = active_turn_id {
            match self.steer_turn(session, &turn_id, message) {
                Ok(()) => return Ok(()),
                Err(error) if is_no_active_turn_error(&error) => {
                    // ponytail: Codex activates a turn a moment after
                    // turn/start's response returns its id (and, symmetrically,
                    // can finish it before its turn/completed notification
                    // reaches us) -- steering in that window races the
                    // server's own "no active turn" check even though our
                    // local state still shows it active. Treat this exactly
                    // like the idle case below: start a fresh turn instead.
                }
                Err(error) => return Err(error),
            }
        }
        self.start_turn(session, message)
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
        let state = self.sessions.get(session).ok_or_else(|| {
            anyhow!("Codex session {session} must be resumed before interrupting")
        })?;
        let thread_id = state.thread_id.clone();
        let turn_id = state
            .active_turn_id
            .clone()
            .ok_or_else(|| anyhow!("Codex session {session} has no active turn to interrupt"))?;
        self.request(
            "turn/interrupt",
            json!({ "threadId": thread_id, "turnId": turn_id }),
        )?;
        if let Some(state) = self.sessions.get_mut(session) {
            state.status = SessionStatus::Terminated(TerminalStatus::Interrupted);
            state.active_turn_id = None;
        }
        Ok(())
    }

    fn inspect(&mut self, session: &GahSessionId) -> Result<SessionStatus> {
        if !self.capabilities.inspect {
            return Err(UnsupportedCapability {
                capability: "inspect",
            }
            .into());
        }
        self.pump()?;
        Ok(self.state_mut(session)?.status.clone())
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

impl Drop for CodexManagerSession {
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
        session: &mut CodexManagerSession,
        id: &GahSessionId,
    ) -> Vec<SessionUpdate> {
        for _ in 0..100 {
            let updates = session.stream(id).unwrap();
            if !updates.is_empty() {
                return updates;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for Codex updates");
    }

    fn wait_for_terminal(session: &mut CodexManagerSession, id: &GahSessionId) -> TerminalStatus {
        for _ in 0..100 {
            if let Some(status) = session.terminal_status(id).unwrap() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for Codex terminal status");
    }

    /// Unlike `wait_for_updates` (which returns as soon as a single
    /// `stream()` call is non-empty), this waits out a short quiet window
    /// after the last update seen -- a burst like "delta then usage" is
    /// emitted via two separate notifications a moment apart, and under
    /// full-suite parallel load those can land in two different `stream()`
    /// polls. Tests that need *all* of a burst (not just proof that
    /// something arrived) should use this instead.
    fn drain_all_pending(
        session: &mut CodexManagerSession,
        id: &GahSessionId,
    ) -> Vec<SessionUpdate> {
        let mut all = Vec::new();
        let mut idle_polls = 0;
        for _ in 0..200 {
            let updates = session.stream(id).unwrap();
            if updates.is_empty() {
                if !all.is_empty() {
                    idle_polls += 1;
                    if idle_polls >= 3 {
                        break;
                    }
                }
            } else {
                idle_polls = 0;
                all.extend(updates);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        all
    }

    /// Shell fragment answering `codex app-server generate-json-schema
    /// --out DIR` the same way `detect_stable_methods` expects the real
    /// binary to: writing a `ClientRequest.json` whose `oneOf[].method`
    /// enums list the given method names. Kept as its own (non-`format!`)
    /// string so its literal JSON braces don't need doubling for the outer
    /// `format!` this gets spliced into.
    fn schema_stub(methods: &[&str]) -> String {
        if methods.is_empty() {
            // Simulates schema generation itself failing (older binary
            // without the subcommand, or any other error) without falling
            // through into the RPC loop below and hanging on stdin.
            return r#"if [ "$1" = "app-server" ] && [ "$2" = "generate-json-schema" ]; then
  exit 1
fi
"#
            .to_string();
        }
        let entries: Vec<String> = methods
            .iter()
            .map(|m| format!(r#"{{"properties":{{"method":{{"enum":["{m}"]}}}}}}"#))
            .collect();
        format!(
            r#"if [ "$1" = "app-server" ] && [ "$2" = "generate-json-schema" ]; then
  shift 2
  outdir=""
  while [ $# -gt 0 ]; do
    if [ "$1" = "--out" ]; then outdir="$2"; shift 2; else shift; fi
  done
  mkdir -p "$outdir"
  cat > "$outdir/ClientRequest.json" <<'JSON'
{{"oneOf":[{entries}]}}
JSON
  exit 0
fi
"#,
            entries = entries.join(",")
        )
    }

    /// A protocol-faithful fake `codex app-server`: real NDJSON framing,
    /// real method names, real response/notification shapes -- verified
    /// against the installed 0.145.0 binary's actual wire behavior (module
    /// doc comment). `turn/start` acks immediately with the turn `inProgress`
    /// and completes asynchronously via notifications on a background
    /// thread, exactly like the real app-server. `stable_methods` controls
    /// what `generate-json-schema` reports, which is what
    /// `detect_stable_methods` uses to set `resume`/`interrupt` capabilities.
    fn make_json_rpc_codex_with_methods(dir: &Path, record_dir: &Path, stable_methods: &[&str]) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'codex-cli 1.2.3'
  exit 0
fi
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  echo 'Logged in using ChatGPT'
  echo 'token=super-secret' >&2
  exit 0
fi
{schema_stub}
if [ "$1" != "app-server" ]; then
  exit 1
fi

tmp="$(mktemp)"
cat > "$tmp" <<'PY'
import json
import os
import sys
import threading
import time

record = os.environ["RECORD_PATH"]
lock = threading.Lock()

def emit(obj):
    with lock:
        print(json.dumps(obj), flush=True)

thread_counter = 0
turn_counter = 0
interrupted_turns = set()

def run_turn(thread_id, turn_id, text):
    emit({{
        "method": "item/agentMessage/delta",
        "params": {{"threadId": thread_id, "turnId": turn_id, "delta": "reply: " + text}},
    }})
    emit({{
        "method": "thread/tokenUsage/updated",
        "params": {{
            "threadId": thread_id,
            "turnId": turn_id,
            "tokenUsage": {{"total": {{"totalTokens": 12}}, "modelContextWindow": 4096}},
        }},
    }})
    if text == "fail":
        emit({{
            "method": "turn/completed",
            "params": {{
                "threadId": thread_id,
                "turn": {{"id": turn_id, "status": "failed", "error": {{"message": "turn failed"}}}},
            }},
        }})
        return
    if text == "slow":
        time.sleep(1)
    with lock:
        if turn_id in interrupted_turns:
            return
    emit({{
        "method": "turn/completed",
        "params": {{"threadId": thread_id, "turn": {{"id": turn_id, "status": "completed", "error": None}}}},
    }})

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
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{}}}})
        elif method == "initialized":
            pass
        elif method == "thread/start":
            thread_counter += 1
            thread_id = "thread-" + str(thread_counter)
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"thread": {{"id": thread_id}}}}}})
        elif method == "thread/resume":
            thread_id = msg["params"]["threadId"]
            if thread_id == "thread-fail":
                emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32000, "message": "resume failed"}}}})
            else:
                emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"thread": {{"id": thread_id}}}}}})
        elif method == "turn/start":
            turn_counter += 1
            turn_id = "turn-" + str(turn_counter)
            thread_id = msg["params"]["threadId"]
            text = msg["params"]["input"][0]["text"]
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"turn": {{"id": turn_id, "status": "inProgress"}}}}}})
            threading.Thread(target=run_turn, args=(thread_id, turn_id, text), daemon=True).start()
        elif method == "turn/steer":
            thread_id = msg["params"]["threadId"]
            expected_turn_id = msg["params"]["expectedTurnId"]
            text = msg["params"]["input"][0]["text"]
            # Real Codex returns the *same* turnId from a successful steer
            # (verified live) and errors "no active turn to steer" once the
            # turn has ended -- the magic "steer-ok" text is this fake's
            # stand-in for "the turn is still genuinely active".
            if text == "steer-ok":
                emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"turnId": expected_turn_id}}}})
                emit({{
                    "method": "item/agentMessage/delta",
                    "params": {{"threadId": thread_id, "turnId": expected_turn_id, "delta": "steered: " + text}},
                }})
            else:
                emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32600, "message": "no active turn to steer"}}}})
        elif method == "turn/interrupt":
            thread_id = msg["params"]["threadId"]
            turn_id = msg["params"]["turnId"]
            with lock:
                interrupted_turns.add(turn_id)
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{}}}})
            emit({{
                "method": "turn/completed",
                "params": {{"threadId": thread_id, "turn": {{"id": turn_id, "status": "interrupted", "error": None}}}},
            }})
        else:
            emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32601, "message": "unknown method"}}}})
PY
export RECORD_PATH='{record_path}'
exec python3 -u "$tmp" "$@"
"#,
            schema_stub = schema_stub(stable_methods),
            record_path = record_dir.join("requests.jsonl").display()
        );
        make_fake_bin(dir, "codex", &body);
    }

    /// Default fake: reports every stable method this adapter uses, so
    /// existing tests keep exercising the "fully capable" path unless they
    /// opt into a narrower method set via `make_json_rpc_codex_with_methods`.
    fn make_json_rpc_codex(dir: &Path, record_dir: &Path) {
        make_json_rpc_codex_with_methods(
            dir,
            record_dir,
            &[
                "thread/start",
                "thread/resume",
                "turn/start",
                "turn/steer",
                "turn/interrupt",
            ],
        );
    }

    #[test]
    fn discover_reports_version_and_redacted_auth_state() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "codex",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"login\" ] && [ \"$2\" = \"status\" ]; then echo 'Logged in using ChatGPT'; echo 'token=super-secret' >&2; exit 0; fi\nexit 1\n",
        );

        let discovery = discover(f.bin_dir.join("codex")).unwrap();
        assert_eq!(discovery.version.as_deref(), Some("codex-cli 1.2.3"));
        assert_eq!(discovery.auth_state, CodexAuthState::LoggedIn);
    }

    #[test]
    fn discovery_does_not_retain_failed_status_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "codex",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\necho 'Authorization: Bearer sk-secret' >&2\nexit 1\n",
        );

        let discovery = discover(f.bin_dir.join("codex")).unwrap();
        let CodexAuthState::Error(message) = discovery.auth_state else {
            panic!("expected failed status discovery");
        };
        assert!(!message.contains("sk-secret"));
    }

    #[test]
    fn session_directory_requires_absolute_user_state() {
        assert_eq!(
            resolve_session_dir(Some(OsStr::new("/state")), Some(OsStr::new("/home/user")))
                .unwrap(),
            PathBuf::from("/state/gah/manager-sessions/codex")
        );
        assert_eq!(
            resolve_session_dir(Some(OsStr::new("")), Some(OsStr::new("/home/user"))).unwrap(),
            PathBuf::from("/home/user/.local/state/gah/manager-sessions/codex")
        );
        assert!(resolve_session_dir(Some(OsStr::new("relative")), None).is_err());
    }

    #[test]
    fn rpc_errors_surface_structured_detail() {
        let error = decode_json_rpc_response(json!({
            "error": {"code": -32000, "message": "thread not found"}
        }))
        .unwrap_err();
        assert_eq!(error.to_string(), "thread not found (code -32000)");
    }

    #[test]
    fn adapter_runs_turn_and_streams_message_chunks() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);

        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
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
    fn turn_returns_before_completion_and_can_be_interrupted() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
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
    fn turn_failure_becomes_terminal_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
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
            panic!("expected failed turn status");
        };
        assert!(message.contains("turn failed"));
    }

    #[test]
    fn failed_mapping_commit_interrupts_the_remote_turn_before_discarding_the_session() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("not-a-directory");
        fs::write(&session_dir, "occupied").unwrap();
        let mut session =
            CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir)
                .unwrap();

        assert!(session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .is_err());
        assert!(session.sessions.is_empty());

        // The turn/start on Codex's side genuinely succeeded (that's the
        // whole problem persist_mapping's failure creates) -- the adapter
        // must have followed up with a real turn/interrupt request rather
        // than just dropping local state and abandoning it.
        for _ in 0..100 {
            let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
            if requests.contains("turn/interrupt") {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("Codex did not receive turn/interrupt after mapping-commit failure");
    }

    #[test]
    fn failed_initial_turn_start_does_not_leave_an_orphaned_local_session() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        session.transport.fail_request = Some("turn/start");

        assert!(session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .is_err());
        assert!(session.sessions.is_empty());
    }

    #[test]
    fn adapter_resume_and_interrupt_round_trip() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);

        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
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
    fn send_steers_an_active_turn_instead_of_starting_a_new_one() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .unwrap();
        drain_all_pending(&mut session, &id); // drain the initial turn's own reply/usage

        session.send(&id, "steer-ok").unwrap();
        let updates = drain_all_pending(&mut session, &id);
        assert_eq!(
            updates,
            vec![SessionUpdate::MessageChunk("steered: steer-ok".into())]
        );

        let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
        assert_eq!(
            requests.matches("\"turn/steer\"").count(),
            1,
            "send against an active turn must use turn/steer exactly once"
        );
        assert_eq!(
            requests.matches("\"turn/start\"").count(),
            1,
            "the steered message must not additionally start a fresh turn -- only the \
             initial start() call's turn/start should appear"
        );
    }

    #[test]
    fn send_falls_back_to_turn_start_when_steer_races_against_turn_completion() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .unwrap();
        drain_all_pending(&mut session, &id);

        // Any text other than the fake's "steer-ok" magic value makes the
        // fake respond to turn/steer the way the real app-server does once
        // a turn has already ended: "no active turn to steer". `send` must
        // recover from that by starting a fresh turn, not by propagating
        // the error.
        session.send(&id, "not-active-anymore").unwrap();
        let updates = drain_all_pending(&mut session, &id);
        assert_eq!(
            updates,
            vec![
                SessionUpdate::MessageChunk("reply: not-active-anymore".into()),
                SessionUpdate::Usage {
                    used: 12,
                    size: 4096
                }
            ]
        );

        let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
        assert!(
            requests.contains("turn/steer"),
            "the race-triggering turn/steer attempt must still have been sent"
        );
    }

    #[test]
    fn capabilities_reflect_methods_the_installed_binary_actually_reports() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // No thread/resume, no turn/interrupt in the fake's schema output --
        // an older/narrower app-server than the one this module was
        // verified against.
        make_json_rpc_codex_with_methods(
            &f.bin_dir,
            &f.record_dir,
            &["thread/start", "turn/start", "turn/steer"],
        );
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();

        assert_eq!(
            session.capabilities(),
            SessionCapabilities {
                resume: false,
                interrupt: false,
                inspect: true,
            }
        );

        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        assert!(
            crate::manager::unsupported_capability(&session.resume(&id).unwrap_err()).is_some()
        );
        assert!(
            crate::manager::unsupported_capability(&session.interrupt(&id).unwrap_err()).is_some()
        );
    }

    #[test]
    fn capabilities_fail_closed_when_schema_detection_itself_fails() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // `generate-json-schema` exits nonzero (e.g. a Codex old enough the
        // subcommand doesn't behave as expected) while the app-server RPC
        // loop itself still works fine -- construction must still succeed,
        // but with every schema-dependent capability honestly false rather
        // than guessed true.
        make_json_rpc_codex_with_methods(&f.bin_dir, &f.record_dir, &[]);
        let session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();

        assert_eq!(
            session.capabilities(),
            SessionCapabilities {
                resume: false,
                interrupt: false,
                inspect: true,
            }
        );
    }

    #[test]
    fn adapter_passes_shared_contract_suite() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        crate::manager::contract::run_contract_suite(&mut session);
    }

    #[test]
    fn restart_resumes_through_durable_gah_to_codex_mapping() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let id = {
            let mut adapter =
                CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir)
                    .unwrap();
            adapter
                .start(StartRequest {
                    profile: "profile-a".into(),
                    instruction: "hello".into(),
                })
                .unwrap()
        };
        assert!(!id.as_str().contains("thread-"));

        let restored_id = id.as_str().parse::<GahSessionId>().unwrap();
        let mut restarted =
            CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir)
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
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let id = GahSessionId::new("profile-a");
        persist_mapping(&session_dir, &id, "thread-fail").unwrap();
        let mut restarted =
            CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir)
                .unwrap();

        assert!(restarted.resume(&id).is_err());
        assert!(restarted
            .send(&id, "must not bypass resume")
            .unwrap_err()
            .to_string()
            .contains("must be resumed"));
    }

    /// Bounded handshake smoke test against whatever `codex` binary is
    /// actually installed -- proves the wire protocol this module hardcodes
    /// (method names, param shapes, response shapes) still matches a real
    /// `codex app-server`, without running a real model turn (no
    /// `turn/start`), so it doesn't spend API budget on every run. Gated
    /// behind an env var, like `installed_hermes_passes_shared_contract_when_requested`
    /// in `manager::hermes`, since most CI environments won't have `codex`
    /// installed or authenticated.
    #[test]
    fn installed_codex_passes_handshake_smoke_when_requested() {
        let Some(executable) = std::env::var_os("GAH_TEST_REAL_CODEX") else {
            return;
        };
        let _exec_guard = crate::test_support::ExecGuard::new();
        let discovery = discover(&executable).unwrap();
        assert!(
            discovery.version.is_some(),
            "installed Codex must report a version"
        );

        let mut transport = CodexTransport::spawn(Path::new(&executable)).unwrap();
        rpc_request(
            &mut transport,
            "initialize",
            json!({
                "clientInfo": {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )
        .unwrap();
        transport
            .send_notification("initialized", json!({}))
            .unwrap();

        let response = rpc_request(
            &mut transport,
            "thread/start",
            json!({
                "cwd": std::env::temp_dir(),
                "approvalPolicy": "never",
            }),
        )
        .unwrap();
        let thread_id = response
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str);
        assert!(
            thread_id.is_some(),
            "real Codex app-server must return a thread id from thread/start"
        );

        let _ = crate::runner::process::kill_process_group(&mut transport.child);
        let _ = transport.child.wait();
    }

    /// Drains `stream()` until `terminal_status()` reports terminal,
    /// accumulating every update seen along the way. Bounded by `timeout`
    /// rather than the fake-server tests' fixed 100x10ms budget, since a
    /// real model turn's latency is real network+inference time, not a
    /// background thread we control.
    fn drain_until_terminal(
        session: &mut CodexManagerSession,
        id: &GahSessionId,
        timeout: Duration,
    ) -> (Vec<SessionUpdate>, TerminalStatus) {
        let deadline = Instant::now() + timeout;
        let mut updates = Vec::new();
        loop {
            updates.extend(session.stream(id).unwrap());
            if let Some(status) = session.terminal_status(id).unwrap() {
                updates.extend(session.stream(id).unwrap());
                return (updates, status);
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for real Codex turn to reach a terminal status");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Bounded real-Codex lifecycle smoke test, gated the same way as
    /// `installed_codex_passes_handshake_smoke_when_requested` above but
    /// going through `CodexManagerSession` itself (not raw transport) for a
    /// real `turn/start`: structured message-chunk and token-usage
    /// streaming, a `Completed` terminal status, a full process restart
    /// resuming purely from the durable on-disk thread mapping, a second
    /// real turn after that resume, and a real `turn/interrupt` against a
    /// turn proven active by already having streamed real content (the
    /// module doc comment's live handshake found interrupting *before* any
    /// content streams back can race Codex's own turn-activation and
    /// return "no active turn to interrupt" -- waiting for the first delta
    /// is the same margin that trace needed).
    ///
    /// `turn/steer` is deliberately not exercised here: doing so
    /// deterministically needs the same wait-for-first-token pattern
    /// layered on top of a *second* concurrently in-flight turn, which
    /// risks turning this bounded, budget-conscious smoke test into a flaky
    /// and budget-unbounded one. It was instead verified directly against
    /// the wire during development: a real `turn/steer` sent after the
    /// turn had streamed content succeeded, returning the same `turnId`
    /// (not a new one) with the turn continuing to stream under it, and a
    /// `turn/steer` sent before any content streamed back reproducibly hit
    /// the same "no active turn to steer" race `send`'s fallback handles.
    #[test]
    fn installed_codex_adapter_completes_a_real_turn_and_resumes_after_restart_when_requested() {
        let Some(executable) = std::env::var_os("GAH_TEST_REAL_CODEX") else {
            return;
        };
        let _exec_guard = crate::test_support::ExecGuard::new();
        let state = tempfile::TempDir::new().unwrap();
        let session_dir = state.path().join("manager-sessions");
        let turn_timeout = Duration::from_secs(30);

        let id = {
            let mut session =
                CodexManagerSession::new_with_session_dir(&executable, &session_dir).unwrap();
            let id = session
                .start(StartRequest {
                    profile: "gah-test-real-codex".into(),
                    instruction: "Reply with exactly: pong".into(),
                })
                .unwrap();
            let (updates, status) = drain_until_terminal(&mut session, &id, turn_timeout);
            assert_eq!(status, TerminalStatus::Completed);
            assert!(
                updates.iter().any(
                    |u| matches!(u, SessionUpdate::MessageChunk(text) if text.contains("pong"))
                ),
                "expected a real agent-message delta containing 'pong', got {updates:?}"
            );
            assert!(
                updates
                    .iter()
                    .any(|u| matches!(u, SessionUpdate::Usage { .. })),
                "expected a real token-usage update, got {updates:?}"
            );
            id
        };

        // Restart: drop the adapter/process entirely, resume purely from
        // the durable GAH-session-id -> Codex-thread-id mapping on disk.
        let mut restarted =
            CodexManagerSession::new_with_session_dir(&executable, &session_dir).unwrap();
        restarted.resume(&id).unwrap();
        restarted.send(&id, "Reply with exactly: ping").unwrap();
        let (updates, status) = drain_until_terminal(&mut restarted, &id, turn_timeout);
        assert_eq!(status, TerminalStatus::Completed);
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, SessionUpdate::MessageChunk(text) if text.contains("ping"))),
            "expected a real agent-message delta containing 'ping' after restart+resume, got {updates:?}"
        );

        // Interrupt: only sent once real content has streamed back, so the
        // turn is unambiguously active server-side (see doc comment above).
        let interrupt_id = restarted
            .start(StartRequest {
                profile: "gah-test-real-codex".into(),
                instruction: "Count slowly from 1 to 500, one number per line, no other text."
                    .into(),
            })
            .unwrap();
        let deadline = Instant::now() + turn_timeout;
        loop {
            let updates = restarted.stream(&interrupt_id).unwrap();
            if updates
                .iter()
                .any(|u| matches!(u, SessionUpdate::MessageChunk(_)))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for real Codex to stream before interrupting"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        restarted.interrupt(&interrupt_id).unwrap();
        let (_, status) = drain_until_terminal(&mut restarted, &interrupt_id, turn_timeout);
        assert_eq!(status, TerminalStatus::Interrupted);
    }
}
