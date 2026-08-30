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
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("opening Codex session map directory {}", path.display()))?
        .sync_all()
        .with_context(|| format!("syncing Codex session map directory {}", path.display()))
}

fn persist_mapping(session_dir: &Path, session: &GahSessionId, thread_id: &str) -> Result<()> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating Codex session map {}", session_dir.display()))?;
    let path = mapping_path(session_dir, session);
    let temp = path.with_extension(format!("json.tmp.{}", uuid::Uuid::new_v4()));
    let mapping = DurableThreadMapping {
        gah_session_id: session.as_str().to_string(),
        thread_id: thread_id.to_string(),
    };
    let mut renamed = false;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .with_context(|| format!("securely creating Codex session map {}", temp.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("securing Codex session map {}", temp.display()))?;
        serde_json::to_writer(&mut file, &mapping).context("serializing Codex session map")?;
        file.sync_all()
            .with_context(|| format!("syncing Codex session map {}", temp.display()))?;
        drop(file);
        fs::rename(&temp, &path)
            .with_context(|| format!("committing Codex session map {}", path.display()))?;
        renamed = true;
        sync_directory(session_dir)?;
        Ok(())
    })();

    match result {
        Ok(()) => Ok(()),
        Err(error) => match cleanup_failed_mapping(session_dir, &temp, &path, renamed) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(anyhow!(
                "{error:#}; additionally failed to clean up the Codex session map: {cleanup:#}"
            )),
        },
    }
}

fn cleanup_failed_mapping(
    session_dir: &Path,
    temp: &Path,
    path: &Path,
    renamed: bool,
) -> Result<()> {
    let mut errors = Vec::new();
    let mut removed = false;
    for candidate in [temp, path].into_iter().take(if renamed { 2 } else { 1 }) {
        match fs::remove_file(candidate) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("removing {}: {error}", candidate.display())),
        }
    }
    if removed {
        if let Err(error) = sync_directory(session_dir) {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}

fn remove_mapping(session_dir: &Path, session: &GahSessionId) -> Result<()> {
    let path = mapping_path(session_dir, session);
    match fs::remove_file(&path) {
        Ok(()) => sync_directory(session_dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("removing Codex session map {}", path.display()))
        }
    }
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
    terminated: bool,
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
            terminated: false,
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

    /// Stops and reaps the app-server exactly once, including constructor rollback.
    fn terminate(&mut self) -> Result<()> {
        if self.terminated {
            return Ok(());
        }
        let cleanup_error = crate::runner::process::kill_process_group(&mut self.child);
        let wait_result = self.child.wait();
        if wait_result.is_ok() {
            self.terminated = true;
        }
        match (cleanup_error, wait_result) {
            (None, Ok(_)) => Ok(()),
            (Some(cleanup), Ok(_)) => Err(anyhow!(cleanup)),
            (None, Err(wait)) => Err(wait).context("waiting for terminated Codex app-server"),
            (Some(cleanup), Err(wait)) => Err(anyhow!(
                "{cleanup}; waiting for terminated Codex app-server also failed: {wait}"
            )),
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

impl Drop for CodexTransport {
    fn drop(&mut self) {
        let _ = self.terminate();
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

#[derive(Debug)]
enum TurnStartFailure {
    BeforeSend(anyhow::Error),
    Rejected(anyhow::Error),
    Ambiguous(anyhow::Error),
}

impl TurnStartFailure {
    fn may_have_started(&self) -> bool {
        matches!(self, Self::Ambiguous(_))
    }

    fn into_error(self) -> anyhow::Error {
        match self {
            Self::BeforeSend(error) | Self::Rejected(error) | Self::Ambiguous(error) => error,
        }
    }
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
                if let (Some(turn_id), Some(state)) =
                    (turn_id, self.session_by_thread_mut(thread_id))
                {
                    // A notification for an older turn may arrive after a
                    // replacement turn has already been acknowledged. Never
                    // let it steal lifecycle ownership from the current turn.
                    if state.active_turn_id.as_deref().is_none()
                        || state.active_turn_id.as_deref() == Some(turn_id.as_str())
                    {
                        state.status = SessionStatus::Working;
                        state.active_turn_id = Some(turn_id);
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
                let Some(turn_id) = turn.get("id").and_then(Value::as_str) else {
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
                        if state.active_turn_id.as_deref() != Some(turn_id) {
                            return Ok(());
                        }
                        state.status = SessionStatus::Terminated(terminal);
                        state.active_turn_id = None;
                    }
                }
            }
            "error" => {
                let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
                    return Ok(());
                };
                let Some(turn_id) = params.get("turnId").and_then(Value::as_str) else {
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
                    if state.active_turn_id.as_deref() != Some(turn_id) {
                        return Ok(());
                    }
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

    /// Starts a turn while preserving whether failure happened before delivery
    /// or after Codex may already have accepted remote work.
    fn start_turn(
        &mut self,
        session: &GahSessionId,
        message: &str,
    ) -> std::result::Result<(), TurnStartFailure> {
        let thread_id = self
            .sessions
            .get(session)
            .map(|state| state.thread_id.clone())
            .ok_or_else(|| {
                TurnStartFailure::BeforeSend(anyhow!(
                    "Codex session {session} must be resumed before sending"
                ))
            })?;
        #[cfg(test)]
        if self.transport.fail_request == Some("turn/start") {
            self.transport.fail_request = None;
            return Err(TurnStartFailure::BeforeSend(anyhow!(
                "injected turn/start enqueue failure"
            )));
        }
        let request_id = self
            .transport
            .send_request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "input": [{"type": "text", "text": message}],
                }),
            )
            .map_err(TurnStartFailure::Ambiguous)?;
        let response = loop {
            let response = self.transport.recv().map_err(TurnStartFailure::Ambiguous)?;
            if is_response_for(&response, request_id) {
                break decode_json_rpc_response(response).map_err(TurnStartFailure::Rejected)?;
            }
            self.handle_message(response)
                .map_err(TurnStartFailure::Ambiguous)?;
        };
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

    fn stop_ambiguous_turn(&mut self, session: &GahSessionId) -> Result<()> {
        let active = self.sessions.get(session).and_then(|state| {
            state
                .active_turn_id
                .as_ref()
                .map(|turn_id| (state.thread_id.clone(), turn_id.clone()))
        });
        let interrupt_error = active.and_then(|(thread_id, turn_id)| {
            self.request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
            )
            .err()
        });
        match (interrupt_error, self.transport.terminate()) {
            (_, Ok(())) => Ok(()),
            (None, Err(terminate)) => Err(terminate),
            (Some(interrupt), Err(terminate)) => Err(anyhow!(
                "interrupting an ambiguously accepted Codex turn failed: {interrupt:#}; transport shutdown also failed: {terminate:#}"
            )),
        }
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
        // Commit the only restart-resume state before spending provider
        // work. If this fails, no turn has been launched and returning no
        // GAH session ID cannot orphan a paid remote operation.
        if let Err(error) = persist_mapping(&self.session_dir, &gah_session_id, &thread_id) {
            self.sessions.remove(&gah_session_id);
            return Err(error);
        }
        if let Err(failure) = self.start_turn(&gah_session_id, &request.instruction) {
            let remote_cleanup = if failure.may_have_started() {
                self.stop_ambiguous_turn(&gah_session_id)
            } else {
                Ok(())
            };
            let rollback = remove_mapping(&self.session_dir, &gah_session_id);
            self.sessions.remove(&gah_session_id);
            let error = failure.into_error();
            return match (remote_cleanup, rollback) {
                (Ok(()), Ok(())) => Err(error)
                    .with_context(|| format!("starting Codex turn on thread {thread_id}")),
                (Err(cleanup), Ok(())) => Err(anyhow!(
                    "starting Codex turn on thread {thread_id}: {error:#}; additionally failed to stop ambiguous remote work: {cleanup:#}"
                )),
                (Ok(()), Err(rollback)) => Err(anyhow!(
                    "starting Codex turn on thread {thread_id}: {error:#}; additionally failed to remove its session mapping: {rollback:#}"
                )),
                (Err(cleanup), Err(rollback)) => Err(anyhow!(
                    "starting Codex turn on thread {thread_id}: {error:#}; additionally failed to stop ambiguous remote work: {cleanup:#}; mapping rollback also failed: {rollback:#}"
                )),
            };
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
            .map_err(TurnStartFailure::into_error)
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

#[cfg(test)]
mod tests;
