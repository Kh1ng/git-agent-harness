//! Claude Code-backed `ManagerSession` adapter.
//!
//! Claude's supported automation surface is a long-lived `--print` process
//! with JSONL input/output. New conversations are pinned with `--session-id`;
//! later adapter instances reconnect with `--resume`. Only documented
//! structured events are normalized here -- never terminal presentation text.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeAuthState {
    LoggedIn {
        auth_method: Option<String>,
        api_provider: Option<String>,
        subscription_type: Option<String>,
    },
    LoggedOut,
    Unknown,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDiscovery {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub auth_state: ClaudeAuthState,
}

pub fn discover(executable: impl AsRef<Path>) -> Result<ClaudeDiscovery> {
    let executable = executable.as_ref().to_path_buf();
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
        });
    let auth_state = discover_auth_state(&executable);
    Ok(ClaudeDiscovery {
        executable,
        version,
        auth_state,
    })
}

#[derive(Deserialize)]
struct AuthStatus {
    #[serde(default, rename = "loggedIn")]
    logged_in: Option<bool>,
    #[serde(default, rename = "authMethod")]
    auth_method: Option<String>,
    #[serde(default, rename = "apiProvider")]
    api_provider: Option<String>,
    #[serde(default, rename = "subscriptionType")]
    subscription_type: Option<String>,
}

fn discover_auth_state(executable: &Path) -> ClaudeAuthState {
    let output = match Command::new(executable)
        .args(["auth", "status", "--json"])
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            return ClaudeAuthState::Error("auth status command could not be started".into());
        }
    };
    if let Ok(status) = serde_json::from_slice::<AuthStatus>(&output.stdout) {
        return match status.logged_in {
            Some(true) => ClaudeAuthState::LoggedIn {
                auth_method: status.auth_method,
                api_provider: status.api_provider,
                subscription_type: status.subscription_type,
            },
            Some(false) => ClaudeAuthState::LoggedOut,
            None if output.status.success() => ClaudeAuthState::Unknown,
            None => ClaudeAuthState::Error(auth_exit_error(&output.status)),
        };
    }
    if output.status.success() {
        ClaudeAuthState::Unknown
    } else {
        ClaudeAuthState::Error(auth_exit_error(&output.status))
    }
}

fn auth_exit_error(status: &std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "auth status terminated without an exit code".to_string(),
        |code| format!("auth status exited with status {code}"),
    )
}

#[derive(Serialize, Deserialize)]
struct DurableSessionMapping {
    gah_session_id: String,
    provider_session_id: String,
    cwd: PathBuf,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MappingFaultStage {
    Serialize,
    FileSync,
    Rename,
    DirectorySync,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct MappingFault {
    stage: MappingFaultStage,
    cleanup_error: Option<String>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteFaultStage {
    Json,
    Newline,
    Flush,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpawnFault {
    MissingStdin,
    MissingStdout,
}

fn resolve_session_dir(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf> {
    if let Some(dir) = xdg_state_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        return Ok(dir.join("gah").join("manager-sessions").join("claude"));
    }
    if let Some(home) = home.map(Path::new).filter(|path| path.is_absolute()) {
        return Ok(home.join(".local/state/gah/manager-sessions/claude"));
    }
    Err(anyhow!(
        "Claude session persistence requires an absolute XDG_STATE_HOME or HOME"
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

fn with_secondary_error(error: anyhow::Error, label: &str, secondary: Result<()>) -> anyhow::Error {
    match secondary {
        Ok(()) => error,
        Err(secondary) => anyhow!("{error:#}; {label} also failed: {secondary:#}"),
    }
}

fn sync_mapping_dir(session_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(session_dir)
        .with_context(|| {
            format!(
                "opening Claude session map directory {}",
                session_dir.display()
            )
        })?
        .sync_all()
        .with_context(|| {
            format!(
                "syncing Claude session map directory {}",
                session_dir.display()
            )
        })?;
    Ok(())
}

fn remove_mapping_files(session_dir: &Path, paths: &[&Path]) -> Result<()> {
    let mut failure = None;
    for path in paths {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                let error = anyhow!(error)
                    .context(format!("removing Claude session map {}", path.display()));
                failure = Some(match failure {
                    None => error,
                    Some(primary) => {
                        with_secondary_error(primary, "another map removal", Err(error))
                    }
                });
            }
        }
    }
    if let Err(error) = sync_mapping_dir(session_dir) {
        failure = Some(match failure {
            None => error,
            Some(primary) => with_secondary_error(primary, "map directory sync", Err(error)),
        });
    }
    failure.map_or(Ok(()), Err)
}

fn remove_mapping(session_dir: &Path, session: &GahSessionId) -> Result<()> {
    let path = mapping_path(session_dir, session);
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    remove_mapping_files(session_dir, &[&temp, &path])
}

#[cfg(test)]
fn inject_mapping_fault(fault: Option<&MappingFault>, stage: MappingFaultStage) -> Result<()> {
    if fault.is_some_and(|fault| fault.stage == stage) {
        anyhow::bail!("injected {stage:?} failure");
    }
    Ok(())
}

fn persist_mapping(
    session_dir: &Path,
    session: &GahSessionId,
    provider_session_id: &str,
    cwd: &Path,
    #[cfg(test)] fault: Option<&MappingFault>,
) -> Result<()> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating Claude session map {}", session_dir.display()))?;
    let path = mapping_path(session_dir, session);
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut final_committed = false;
    let stored = (|| {
        let mut file = options
            .open(&temp)
            .with_context(|| format!("creating Claude session map {}", temp.display()))?;
        #[cfg(test)]
        inject_mapping_fault(fault, MappingFaultStage::Serialize)
            .context("serializing Claude session map")?;
        serde_json::to_writer(
            &mut file,
            &DurableSessionMapping {
                gah_session_id: session.as_str().to_string(),
                provider_session_id: provider_session_id.to_string(),
                cwd: cwd.to_path_buf(),
            },
        )
        .context("serializing Claude session map")?;
        #[cfg(test)]
        inject_mapping_fault(fault, MappingFaultStage::FileSync)
            .with_context(|| format!("syncing Claude session map {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing Claude session map {}", temp.display()))?;
        #[cfg(test)]
        inject_mapping_fault(fault, MappingFaultStage::Rename)
            .with_context(|| format!("committing Claude session map {}", path.display()))?;
        fs::rename(&temp, &path)
            .with_context(|| format!("committing Claude session map {}", path.display()))?;
        final_committed = true;
        #[cfg(test)]
        inject_mapping_fault(fault, MappingFaultStage::DirectorySync).with_context(|| {
            format!(
                "syncing Claude session map directory {}",
                session_dir.display()
            )
        })?;
        sync_mapping_dir(session_dir)
    })();
    if let Err(error) = stored {
        let cleanup = if final_committed {
            remove_mapping_files(session_dir, &[&path])
        } else {
            remove_mapping_files(session_dir, &[&temp])
        };
        #[cfg(test)]
        let cleanup = match fault.and_then(|fault| fault.cleanup_error.as_deref()) {
            Some(message) => Err(with_secondary_error(
                anyhow!("{message}"),
                "actual map rollback",
                cleanup,
            )),
            None => cleanup,
        };
        return Err(with_secondary_error(
            error,
            "Claude session map rollback",
            cleanup,
        ));
    }
    Ok(())
}

fn load_mapping(session_dir: &Path, session: &GahSessionId) -> Result<DurableSessionMapping> {
    let path = mapping_path(session_dir, session);
    let file = File::open(&path)
        .with_context(|| format!("opening Claude session map {}", path.display()))?;
    let mapping: DurableSessionMapping = serde_json::from_reader(file)
        .with_context(|| format!("parsing Claude session map {}", path.display()))?;
    if mapping.gah_session_id != session.as_str() {
        return Err(anyhow!(
            "Claude session map identity mismatch for {session}"
        ));
    }
    if !mapping.cwd.is_absolute() {
        return Err(anyhow!(
            "Claude session map cwd is not absolute for {session}"
        ));
    }
    uuid::Uuid::parse_str(&mapping.provider_session_id)
        .context("Claude session map contains an invalid provider session UUID")?;
    Ok(mapping)
}

#[derive(Debug)]
struct OwnedChild {
    child: Option<Child>,
    #[cfg(test)]
    injected_cleanup_error: Option<String>,
    #[cfg(test)]
    injected_wait_error: Option<String>,
}

impl OwnedChild {
    fn new(
        child: Child,
        #[cfg(test)] injected_cleanup_error: Option<String>,
        #[cfg(test)] injected_wait_error: Option<String>,
    ) -> Self {
        Self {
            child: Some(child),
            #[cfg(test)]
            injected_cleanup_error,
            #[cfg(test)]
            injected_wait_error,
        }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned Claude child is present")
    }

    fn terminate(&mut self) -> Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let cleanup = terminate_child(&mut child);
        #[cfg(test)]
        let cleanup = match self.injected_cleanup_error.take() {
            Some(error) => Err(with_secondary_error(
                anyhow!(error),
                "actual process cleanup",
                cleanup,
            )),
            None => cleanup,
        };
        #[cfg(test)]
        let cleanup = match self.injected_wait_error.take() {
            Some(error) => match cleanup {
                Ok(()) => Err(anyhow!(error)),
                Err(primary) => Err(with_secondary_error(
                    primary,
                    "Claude child wait",
                    Err(anyhow!(error)),
                )),
            },
            None => cleanup,
        };
        cleanup
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

fn terminate_child(child: &mut Child) -> Result<()> {
    let group_cleanup = crate::runner::process::kill_process_group(child).map(anyhow::Error::msg);
    let wait = child
        .wait()
        .context("waiting for terminated Claude Code child");
    match group_cleanup {
        Some(error) => Err(with_secondary_error(
            error,
            "Claude child wait",
            wait.map(|_| ()),
        )),
        None => wait.map(|_| ()),
    }
}

#[derive(Debug)]
struct ClaudeProcess {
    child: OwnedChild,
    stdin: ChildStdin,
    messages: Receiver<std::result::Result<Value, String>>,
    #[cfg(test)]
    write_fault: Option<WriteFaultStage>,
}

impl ClaudeProcess {
    fn write_user_message(&mut self, session_id: &str, message: &str) -> Result<()> {
        let payload = serde_json::to_vec(&json!({
            "type": "user",
            "message": {"role": "user", "content": message},
            "parent_tool_use_id": null,
            "session_id": session_id,
        }))?;
        #[cfg(test)]
        let fault = self.write_fault.take();
        #[cfg(test)]
        if fault == Some(WriteFaultStage::Json) {
            self.stdin.write_all(&payload[..1])?;
            anyhow::bail!("injected Json write failure");
        }
        self.stdin.write_all(&payload)?;
        #[cfg(test)]
        if fault == Some(WriteFaultStage::Newline) {
            anyhow::bail!("injected Newline write failure");
        }
        self.stdin.write_all(b"\n")?;
        #[cfg(test)]
        if fault == Some(WriteFaultStage::Flush) {
            anyhow::bail!("injected Flush write failure");
        }
        self.stdin.flush()?;
        Ok(())
    }
}

#[derive(Debug)]
struct ClaudeSessionState {
    provider_session_id: String,
    process: Option<ClaudeProcess>,
    pending_updates: Vec<SessionUpdate>,
    status: SessionStatus,
    outstanding_turns: usize,
    saw_text_delta: bool,
    assistant_fallback: Vec<String>,
}

pub struct ClaudeManagerSession {
    discovery: ClaudeDiscovery,
    session_dir: PathBuf,
    sessions: HashMap<GahSessionId, ClaudeSessionState>,
    extra_args: Vec<OsString>,
    #[cfg(test)]
    next_cleanup_error: Option<String>,
    #[cfg(test)]
    next_wait_error: Option<String>,
    #[cfg(test)]
    next_mapping_fault: Option<MappingFault>,
    #[cfg(test)]
    next_spawn_fault: Option<SpawnFault>,
    #[cfg(test)]
    next_write_fault: Option<WriteFaultStage>,
}

impl ClaudeManagerSession {
    pub fn new() -> Result<Self> {
        let executable = crate::runner::resolve::resolve_executable_on_path("claude")
            .context("claude executable not found on PATH")?;
        Self::with_executable(executable)
    }

    pub fn with_executable(executable: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_session_dir(executable, default_session_dir()?)
    }

    pub fn discovery(&self) -> &ClaudeDiscovery {
        &self.discovery
    }

    fn new_with_session_dir(
        executable: impl AsRef<Path>,
        session_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        Self::new_with_session_dir_and_args(executable, session_dir, Vec::new())
    }

    fn new_with_session_dir_and_args(
        executable: impl AsRef<Path>,
        session_dir: impl Into<PathBuf>,
        extra_args: Vec<OsString>,
    ) -> Result<Self> {
        let executable = executable.as_ref();
        if !crate::runner::is_executable_path(executable) {
            anyhow::bail!(
                "configured executable '{}' does not exist or is not executable",
                executable.display()
            );
        }
        Ok(Self {
            discovery: discover(executable)?,
            session_dir: session_dir.into(),
            sessions: HashMap::new(),
            extra_args,
            #[cfg(test)]
            next_cleanup_error: None,
            #[cfg(test)]
            next_wait_error: None,
            #[cfg(test)]
            next_mapping_fault: None,
            #[cfg(test)]
            next_spawn_fault: None,
            #[cfg(test)]
            next_write_fault: None,
        })
    }

    fn spawn_process(
        &mut self,
        provider_session_id: &str,
        cwd: &Path,
        resume: bool,
    ) -> Result<ClaudeProcess> {
        let mut command = Command::new(&self.discovery.executable);
        command
            .arg("--print")
            .arg("--verbose")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--permission-mode")
            .arg("dontAsk");
        if resume {
            command.arg("--resume");
        } else {
            command.arg("--session-id");
        }
        command
            .arg(provider_session_id)
            .args(&self.extra_args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::runner::process::prepare_process_group(&mut command);
        let child = command.spawn().with_context(|| {
            format!(
                "launching Claude Code from {}",
                self.discovery.executable.display()
            )
        })?;
        let mut child = OwnedChild::new(
            child,
            #[cfg(test)]
            self.next_cleanup_error.take(),
            #[cfg(test)]
            self.next_wait_error.take(),
        );
        #[cfg(test)]
        match self.next_spawn_fault.take() {
            Some(SpawnFault::MissingStdin) => drop(child.child_mut().stdin.take()),
            Some(SpawnFault::MissingStdout) => drop(child.child_mut().stdout.take()),
            None => {}
        }
        let stdin = match child.child_mut().stdin.take() {
            Some(stdin) => stdin,
            None => {
                return Err(Self::with_cleanup_error(
                    anyhow!("Claude Code child did not provide stdin"),
                    child.terminate(),
                ));
            }
        };
        let stdout = match child.child_mut().stdout.take() {
            Some(stdout) => stdout,
            None => {
                return Err(Self::with_cleanup_error(
                    anyhow!("Claude Code child did not provide stdout"),
                    child.terminate(),
                ));
            }
        };
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || read_messages(stdout, sender));
        Ok(ClaudeProcess {
            child,
            stdin,
            messages,
            #[cfg(test)]
            write_fault: self.next_write_fault.take(),
        })
    }

    fn terminate_process(process: &mut Option<ClaudeProcess>) -> Result<()> {
        let Some(mut process) = process.take() else {
            return Ok(());
        };
        process.child.terminate()
    }

    fn with_cleanup_error(error: anyhow::Error, cleanup: Result<()>) -> anyhow::Error {
        with_secondary_error(error, "Claude process cleanup", cleanup)
    }

    fn fail_session(state: &mut ClaudeSessionState, error: String) -> Result<()> {
        let failure =
            Self::with_cleanup_error(anyhow!(error), Self::terminate_process(&mut state.process));
        state.status = SessionStatus::Terminated(TerminalStatus::Failed(failure.to_string()));
        state.outstanding_turns = 0;
        Err(failure)
    }

    fn pump(&mut self, session: &GahSessionId) -> Result<()> {
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("Claude session {session} must be resumed before use"))?;
        let mut disconnected = false;
        while let Some(process) = state.process.as_ref() {
            match process.messages.try_recv() {
                Ok(Ok(message)) => {
                    if let Err(error) = handle_message(state, message) {
                        return Self::fail_session(state, error.to_string());
                    }
                }
                Ok(Err(error)) => return Self::fail_session(state, error),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            if state.outstanding_turns > 0 {
                return Self::fail_session(
                    state,
                    "Claude Code closed its structured output before a result event".into(),
                );
            }
            if let Err(error) = Self::terminate_process(&mut state.process) {
                return Self::fail_session(
                    state,
                    format!("retiring closed Claude Code transport: {error:#}"),
                );
            }
        }
        Ok(())
    }
}

fn read_messages(stdout: ChildStdout, sender: mpsc::Sender<std::result::Result<Value, String>>) {
    for line in BufReader::new(stdout).lines() {
        let message = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => serde_json::from_str(line.trim())
                .map_err(|error| format!("parsing Claude stream-json event: {error}")),
            Err(error) => Err(format!("reading Claude stream-json output: {error}")),
        };
        // A malformed/unreadable line means the wire protocol is no longer
        // trustworthy: stop relaying further output rather than risk
        // resurrecting a dead session with a later well-formed message.
        let stop = message.is_err();
        if sender.send(message).is_err() || stop {
            return;
        }
    }
}

fn handle_message(state: &mut ClaudeSessionState, message: Value) -> Result<()> {
    let kind = message
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Claude message is missing string type"))?;
    if kind == "rate_limit_event" {
        return Ok(());
    }
    if !matches!(
        kind,
        "system" | "user" | "stream_event" | "assistant" | "result"
    ) {
        return Err(anyhow!("unknown Claude message type {kind}"));
    }
    let returned_session_id = message
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Claude {kind} message is missing session_id"))?;
    if returned_session_id != state.provider_session_id {
        return Err(anyhow!(
            "Claude returned session ID {returned_session_id} for expected session {}",
            state.provider_session_id
        ));
    }
    let nested = message
        .get("parent_tool_use_id")
        .is_some_and(|id| !id.is_null());
    match kind {
        "system" => {
            message
                .get("subtype")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Claude system message is missing subtype"))?;
        }
        "user" => {
            message
                .get("message")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("Claude user message is missing message object"))?;
        }
        "stream_event" => {
            let event = message
                .get("event")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("Claude stream event is missing event object"))?;
            let event_kind = event
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Claude stream event is missing event type"))?;
            if event_kind == "content_block_delta" {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_object)
                    .ok_or_else(|| anyhow!("Claude stream event delta is missing"))?;
                let delta_kind = delta
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Claude stream event delta is missing type"))?;
                if delta_kind == "text_delta" {
                    let text = delta
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("Claude text delta is missing text"))?;
                    if !nested && !text.is_empty() {
                        state
                            .pending_updates
                            .push(SessionUpdate::MessageChunk(text.to_string()));
                        state.saw_text_delta = true;
                    }
                }
            }
        }
        "assistant" => {
            let content = message
                .get("message")
                .and_then(|assistant| assistant.get("content"))
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Claude assistant message.content is missing"))?;
            if !nested {
                state
                    .assistant_fallback
                    .extend(content.iter().filter_map(|block| {
                        (block.get("type").and_then(Value::as_str) == Some("text"))
                            .then(|| block.get("text").and_then(Value::as_str))
                            .flatten()
                            .filter(|text| !text.is_empty())
                            .map(str::to_owned)
                    }));
            }
        }
        "result" => {
            message
                .get("subtype")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Claude result is missing subtype"))?;
            message
                .get("is_error")
                .and_then(Value::as_bool)
                .ok_or_else(|| anyhow!("Claude result is_error is missing"))?;
            if !nested {
                finish_turn(state, &message);
            }
        }
        _ => unreachable!("validated Claude message kind"),
    }
    Ok(())
}

fn finish_turn(state: &mut ClaudeSessionState, result: &Value) {
    if !state.saw_text_delta {
        state.pending_updates.extend(
            state
                .assistant_fallback
                .drain(..)
                .map(SessionUpdate::MessageChunk),
        );
    } else {
        state.assistant_fallback.clear();
    }
    if let Some(usage) = extract_usage(result) {
        state.pending_updates.push(usage);
    }
    state.outstanding_turns = state.outstanding_turns.saturating_sub(1);
    let subtype = result
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let is_error = result
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(subtype != "success");
    let terminal = if subtype == "success" && !is_error {
        TerminalStatus::Completed
    } else {
        TerminalStatus::Failed(structured_error(result, subtype))
    };
    let still_working = state.outstanding_turns > 0;
    match (&state.status, terminal, still_working) {
        (SessionStatus::Terminated(TerminalStatus::Interrupted), _, _) => {}
        (SessionStatus::Terminated(TerminalStatus::Failed(_)), _, _) => {}
        (_, TerminalStatus::Failed(error), _) => {
            state.status = SessionStatus::Terminated(TerminalStatus::Failed(error));
        }
        (_, TerminalStatus::Completed, false) => {
            state.status = SessionStatus::Terminated(TerminalStatus::Completed);
        }
        (_, TerminalStatus::Completed, true) => {
            state.status = SessionStatus::Working;
        }
        // Claude's own result JSON never reports `Interrupted` -- only
        // `interrupt()` produces that variant, directly, outside this match.
        (_, TerminalStatus::Interrupted, _) => {}
    }
    state.saw_text_delta = false;
}

fn extract_usage(result: &Value) -> Option<SessionUpdate> {
    let usage = result.get("usage")?;
    let fallback_used = [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .into_iter()
    .filter_map(|field| usage.get(field).and_then(Value::as_u64))
    .fold(0_u64, u64::saturating_add);
    let model_usage = result.get("modelUsage")?.as_object()?;
    let size = model_usage
        .values()
        .filter_map(|model| model.get("contextWindow").and_then(Value::as_u64))
        .max()?;
    let used = model_usage
        .values()
        .filter_map(|model| model.get("contextWindowUsage").and_then(Value::as_u64))
        .max()
        .unwrap_or(fallback_used);
    Some(SessionUpdate::Usage { used, size })
}

fn structured_error(result: &Value, subtype: &str) -> String {
    let details = result
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .map(crate::redact::redact)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|details| !details.is_empty());
    let http = result
        .get("api_error_status")
        .and_then(Value::as_u64)
        .map(|status| format!("HTTP {status}"));
    let suffix = details.or(http).unwrap_or_else(|| {
        result
            .get("terminal_reason")
            .and_then(Value::as_str)
            .unwrap_or("unspecified failure")
            .to_string()
    });
    format!("Claude structured result {subtype}: {suffix}")
}

impl ManagerSession for ClaudeManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            resume: true,
            interrupt: true,
            inspect: false,
        }
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let cwd = std::env::current_dir().context("resolving cwd for Claude session")?;
        let session = GahSessionId::new(&request.profile);
        let provider_session_id = uuid::Uuid::new_v4().to_string();
        #[cfg(test)]
        let mapping_fault = self.next_mapping_fault.take();
        persist_mapping(
            &self.session_dir,
            &session,
            &provider_session_id,
            &cwd,
            #[cfg(test)]
            mapping_fault.as_ref(),
        )?;
        let process = match self.spawn_process(&provider_session_id, &cwd, false) {
            Ok(process) => process,
            Err(error) => {
                return Err(with_secondary_error(
                    error,
                    "Claude session map rollback",
                    remove_mapping(&self.session_dir, &session),
                ));
            }
        };
        self.sessions.insert(
            session.clone(),
            ClaudeSessionState {
                provider_session_id: provider_session_id.clone(),
                process: Some(process),
                pending_updates: Vec::new(),
                status: SessionStatus::Working,
                outstanding_turns: 1,
                saw_text_delta: false,
                assistant_fallback: Vec::new(),
            },
        );
        let delivered = self
            .sessions
            .get_mut(&session)
            .expect("just inserted")
            .process
            .as_mut()
            .expect("just inserted a running Claude process")
            .write_user_message(&provider_session_id, &request.instruction)
            .context("sending Claude's initial structured user message");
        if let Err(error) = delivered {
            let cleanup = Self::terminate_process(
                &mut self
                    .sessions
                    .get_mut(&session)
                    .expect("just inserted")
                    .process,
            );
            self.sessions.remove(&session);
            let error = Self::with_cleanup_error(error, cleanup);
            return Err(with_secondary_error(
                error,
                "Claude session map rollback",
                remove_mapping(&self.session_dir, &session),
            ));
        }
        Ok(session)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        let mapping = load_mapping(&self.session_dir, session)?;
        if let Some(mut old) = self.sessions.remove(session) {
            Self::terminate_process(&mut old.process)
                .with_context(|| format!("stopping existing Claude session {session}"))?;
        }
        let process = self.spawn_process(&mapping.provider_session_id, &mapping.cwd, true)?;
        self.sessions.insert(
            session.clone(),
            ClaudeSessionState {
                provider_session_id: mapping.provider_session_id,
                process: Some(process),
                pending_updates: Vec::new(),
                status: SessionStatus::Idle,
                outstanding_turns: 0,
                saw_text_delta: false,
                assistant_fallback: Vec::new(),
            },
        );
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        self.pump(session)?;
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("Claude session {session} must be resumed before sending"))?;
        let delivered = match state.process.as_mut() {
            Some(process) => process.write_user_message(&state.provider_session_id, message),
            None => Err(anyhow!(
                "Claude session {session} has no running process; resume it before sending"
            )),
        }
        .with_context(|| format!("sending structured input to Claude session {session}"));
        if let Err(error) = delivered {
            if matches!(
                &state.status,
                SessionStatus::Terminated(TerminalStatus::Failed(_))
                    | SessionStatus::Terminated(TerminalStatus::Interrupted)
            ) {
                return Err(error);
            }
            return Self::fail_session(state, format!("{error:#}"));
        }
        state.outstanding_turns += 1;
        state.status = SessionStatus::Working;
        Ok(())
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        self.pump(session)?;
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("Claude session {session} must be resumed before streaming"))?;
        Ok(std::mem::take(&mut state.pending_updates))
    }

    fn interrupt(&mut self, session: &GahSessionId) -> Result<()> {
        // Claude's stream-json input protocol has no interrupt control
        // message today: `{"type": "interrupt"}` is a still-open feature
        // request (anthropics/claude-code#41665), and probing the installed
        // CLI confirms it is silently ignored -- a turn runs to completion
        // regardless. The only real, verifiable way to stop an in-progress
        // turn is to terminate the child process; the durable session
        // mapping is untouched, so a later `resume` still reconnects.
        let state = self.sessions.get_mut(session).ok_or_else(|| {
            anyhow!("Claude session {session} must be resumed before interrupting")
        })?;
        let result = Self::terminate_process(&mut state.process)
            .with_context(|| format!("interrupting Claude session {session}"));
        state.outstanding_turns = 0;
        match result {
            Ok(()) => {
                state.status = SessionStatus::Terminated(TerminalStatus::Interrupted);
                Ok(())
            }
            Err(error) => {
                state.status = SessionStatus::Terminated(TerminalStatus::Failed(error.to_string()));
                Err(error)
            }
        }
    }

    fn inspect(&mut self, _session: &GahSessionId) -> Result<SessionStatus> {
        Err(UnsupportedCapability {
            capability: "inspect",
        }
        .into())
    }

    fn terminal_status(&mut self, session: &GahSessionId) -> Result<Option<TerminalStatus>> {
        self.pump(session)?;
        let state = self
            .sessions
            .get(session)
            .ok_or_else(|| anyhow!("Claude session {session} must be resumed before use"))?;
        Ok(match &state.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}

impl Drop for ClaudeManagerSession {
    fn drop(&mut self) {
        for state in self.sessions.values_mut() {
            let _ = Self::terminate_process(&mut state.process);
        }
    }
}

#[cfg(test)]
#[path = "claude/tests.rs"]
mod tests;
