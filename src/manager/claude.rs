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

fn persist_mapping(
    session_dir: &Path,
    session: &GahSessionId,
    provider_session_id: &str,
    cwd: &Path,
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
    let mut file = options
        .open(&temp)
        .with_context(|| format!("creating Claude session map {}", temp.display()))?;
    serde_json::to_writer(
        &mut file,
        &DurableSessionMapping {
            gah_session_id: session.as_str().to_string(),
            provider_session_id: provider_session_id.to_string(),
            cwd: cwd.to_path_buf(),
        },
    )
    .context("serializing Claude session map")?;
    file.sync_all()
        .with_context(|| format!("syncing Claude session map {}", temp.display()))?;
    fs::rename(&temp, &path)
        .with_context(|| format!("committing Claude session map {}", path.display()))?;
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
struct ClaudeProcess {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<std::result::Result<Value, String>>,
    #[cfg(test)]
    injected_cleanup_error: Option<String>,
}

impl ClaudeProcess {
    fn write_user_message(&mut self, session_id: &str, message: &str) -> Result<()> {
        serde_json::to_writer(
            &mut self.stdin,
            &json!({
                "type": "user",
                "message": {"role": "user", "content": message},
                "parent_tool_use_id": null,
                "session_id": session_id,
            }),
        )?;
        self.stdin.write_all(b"\n")?;
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
        let mut child = command.spawn().with_context(|| {
            format!(
                "launching Claude Code from {}",
                self.discovery.executable.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Claude Code child did not provide stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Claude Code child did not provide stdout"))?;
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || read_messages(stdout, sender));
        Ok(ClaudeProcess {
            child,
            stdin,
            messages,
            #[cfg(test)]
            injected_cleanup_error: self.next_cleanup_error.take(),
        })
    }

    fn terminate_process(process: &mut Option<ClaudeProcess>) -> Result<()> {
        let Some(mut process) = process.take() else {
            return Ok(());
        };
        let cleanup_error = crate::runner::process::kill_process_group(&mut process.child);
        #[cfg(test)]
        let cleanup_error = cleanup_error.or(process.injected_cleanup_error.take());
        let wait_result = process.child.wait();
        if let Some(error) = cleanup_error {
            return Err(anyhow!(error));
        }
        wait_result.context("waiting for terminated Claude Code child")?;
        Ok(())
    }

    fn with_cleanup_error(error: anyhow::Error, cleanup: Result<()>) -> anyhow::Error {
        match cleanup {
            Ok(()) => error,
            Err(cleanup) => {
                anyhow!("{error:#}; Claude process cleanup also failed: {cleanup:#}")
            }
        }
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
        if disconnected && state.outstanding_turns > 0 {
            return Self::fail_session(
                state,
                "Claude Code closed its structured output before a result event".into(),
            );
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
    if let Some(returned_session_id) = message.get("session_id").and_then(Value::as_str) {
        if returned_session_id != state.provider_session_id {
            return Err(anyhow!(
                "Claude returned session ID {returned_session_id} for expected session {}",
                state.provider_session_id
            ));
        }
    }
    if message
        .get("parent_tool_use_id")
        .is_some_and(|id| !id.is_null())
    {
        return Ok(());
    }
    match message.get("type").and_then(Value::as_str) {
        Some("stream_event") => {
            let event = message.get("event").unwrap_or(&Value::Null);
            if event.get("type").and_then(Value::as_str) == Some("content_block_delta")
                && event
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    == Some("text_delta")
            {
                if let Some(text) = event
                    .get("delta")
                    .and_then(|delta| delta.get("text"))
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    state
                        .pending_updates
                        .push(SessionUpdate::MessageChunk(text.to_string()));
                    state.saw_text_delta = true;
                }
            }
        }
        Some("assistant") => {
            if let Some(content) = message
                .get("message")
                .and_then(|assistant| assistant.get("content"))
                .and_then(Value::as_array)
            {
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
        Some("result") => finish_turn(state, &message),
        _ => {}
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
        let process = self.spawn_process(&provider_session_id, &cwd, false)?;
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
        let started = (|| {
            self.sessions
                .get_mut(&session)
                .expect("just inserted")
                .process
                .as_mut()
                .expect("just inserted a running Claude process")
                .write_user_message(&provider_session_id, &request.instruction)
                .context("sending Claude's initial structured user message")?;
            persist_mapping(&self.session_dir, &session, &provider_session_id, &cwd)
        })();
        if let Err(error) = started {
            let cleanup = Self::terminate_process(
                &mut self
                    .sessions
                    .get_mut(&session)
                    .expect("just inserted")
                    .process,
            );
            self.sessions.remove(&session);
            return Err(Self::with_cleanup_error(error, cleanup));
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
        state
            .process
            .as_mut()
            .ok_or_else(|| {
                anyhow!("Claude session {session} has no running process; resume it before sending")
            })?
            .write_user_message(&state.provider_session_id, message)
            .with_context(|| format!("sending structured input to Claude session {session}"))?;
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
mod tests {
    use super::*;
    use crate::manager::{
        unsupported_capability, GahSessionId, ManagerSession, SessionUpdate, StartRequest,
        TerminalStatus,
    };
    use crate::runner::backends::test_util::{fixture, make_fake_bin};
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn discovery_records_only_whitelisted_version_and_auth_fields() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '2.1.197 (Claude Code)'
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "status" ] && [ "$3" = "--json" ]; then
  echo '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"pro","accessToken":"super-secret"}'
  echo 'Authorization: Bearer super-secret' >&2
  exit 0
fi
exit 1
"#,
        );

        let discovery = discover(f.bin_dir.join("claude")).unwrap();
        assert_eq!(discovery.executable, f.bin_dir.join("claude"));
        assert_eq!(discovery.version.as_deref(), Some("2.1.197 (Claude Code)"));
        assert_eq!(
            discovery.auth_state,
            ClaudeAuthState::LoggedIn {
                auth_method: Some("claude.ai".into()),
                api_provider: Some("firstParty".into()),
                subscription_type: Some("pro".into()),
            }
        );
        assert!(!format!("{discovery:?}").contains("super-secret"));
    }

    #[test]
    fn failed_auth_discovery_does_not_retain_command_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\necho 'token=super-secret' >&2\nexit 7\n",
        );

        let discovery = discover(f.bin_dir.join("claude")).unwrap();
        let ClaudeAuthState::Error(message) = discovery.auth_state else {
            panic!("expected a redacted discovery failure");
        };
        assert!(message.contains("status 7"));
        assert!(!message.contains("super-secret"));
    }

    fn make_streaming_claude(dir: &Path, record_dir: &Path) {
        let script = format!(
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

record_path = {record_path:?}
sentinel_path = {sentinel_path:?}
eof_alive_path = {eof_alive_path:?}
args = sys.argv[1:]
if args == ["--version"]:
    print("2.1.197 (Claude Code)")
    raise SystemExit(0)
if args == ["auth", "status", "--json"]:
    print(json.dumps({{
        "loggedIn": True,
        "authMethod": "claude.ai",
        "apiProvider": "firstParty",
        "subscriptionType": "pro",
        "accessToken": "must-not-be-retained",
    }}))
    raise SystemExit(0)

with open(record_path, "a", encoding="utf-8") as record:
    record.write(json.dumps(args) + "\n")
    record.flush()

def arg_value(flag):
    try:
        return args[args.index(flag) + 1]
    except (ValueError, IndexError):
        return None

session_id = arg_value("--session-id") or arg_value("--resume")
print(json.dumps({{
    "type": "system",
    "subtype": "init",
    "session_id": session_id,
    "claude_code_version": "2.1.197",
}}), flush=True)

for raw in sys.stdin:
    message = json.loads(raw)
    prompt = message["message"]["content"]
    if prompt == "malformed":
        print("not-valid-json-garbage", flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "must never be observed",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    if prompt == "mismatched-session":
        print(json.dumps({{
            "type": "assistant",
            "session_id": "00000000-0000-0000-0000-000000000000",
            "message": {{"content": [{{"type": "text", "text": "must never be observed"}}]}},
        }}), flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "must never complete",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    if prompt == "close-output":
        with open(eof_alive_path, "w", encoding="utf-8") as fh:
            fh.write("stdout closed while child remains alive")
        os.close(sys.stdout.fileno())
        time.sleep(10)
        continue
    if prompt == "slow":
        time.sleep(3)
        with open(sentinel_path, "w", encoding="utf-8") as fh:
            fh.write("completed")
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "slow turn completed",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    if prompt == "delayed-fail":
        time.sleep(0.2)
    if prompt == "fail" or prompt == "delayed-fail":
        print(json.dumps({{
            "type": "assistant",
            "session_id": session_id,
            "message": {{"content": [{{"type": "text", "text": "presentation failure prose"}}]}},
        }}), flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": True,
            "session_id": session_id,
            "errors": ["structured protocol failure"],
            "usage": {{"input_tokens": 10, "output_tokens": 2,
                       "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "model_error",
        }}), flush=True)
        continue
    reply = "reply: " + prompt
    print(json.dumps({{
        "type": "stream_event",
        "session_id": session_id,
        "event": {{"type": "content_block_delta",
                  "delta": {{"type": "text_delta", "text": reply}}}},
    }}), flush=True)
    print(json.dumps({{
        "type": "assistant",
        "session_id": session_id,
        "message": {{"content": [{{"type": "text", "text": reply}}]}},
    }}), flush=True)
    print(json.dumps({{
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "session_id": session_id,
        "result": reply,
        "usage": {{"input_tokens": 10, "output_tokens": 2,
                   "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}},
        "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
        "terminal_reason": "completed",
    }}), flush=True)
"#,
            record_path = record_dir.join("argv.jsonl").display().to_string(),
            sentinel_path = record_dir
                .join("slow-completed.marker")
                .display()
                .to_string(),
            eof_alive_path = record_dir
                .join("eof-child-alive.marker")
                .display()
                .to_string(),
        );
        make_fake_bin(dir, "claude", &script);
    }

    fn wait_for_turn(
        session: &mut ClaudeManagerSession,
        id: &GahSessionId,
    ) -> (Vec<SessionUpdate>, TerminalStatus) {
        let mut updates = Vec::new();
        for _ in 0..200 {
            updates.extend(session.stream(id).unwrap());
            if let Some(status) = session.terminal_status(id).unwrap() {
                updates.extend(session.stream(id).unwrap());
                return (updates, status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for Claude's structured result event");
    }

    #[test]
    fn structured_stream_drives_output_usage_and_completed_lifecycle() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();

        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        let (updates, status) = wait_for_turn(&mut adapter, &id);

        assert_eq!(status, TerminalStatus::Completed);
        assert_eq!(
            updates,
            vec![
                SessionUpdate::MessageChunk("reply: hello".into()),
                SessionUpdate::Usage {
                    used: 19,
                    size: 200_000,
                },
            ]
        );
    }

    #[test]
    fn structured_result_error_becomes_terminal_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "fail".into(),
            })
            .unwrap();

        let (_, TerminalStatus::Failed(message)) = wait_for_turn(&mut adapter, &id) else {
            panic!("expected Claude's structured error result to fail the turn");
        };
        assert!(message.contains("error_during_execution"));
        assert!(message.contains("structured protocol failure"));
        assert!(!message.contains("presentation failure prose"));
    }

    #[test]
    fn restart_resumes_with_the_durable_provider_session_id() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("sessions");
        let id = {
            let mut adapter =
                ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                    .unwrap();
            let id = adapter
                .start(StartRequest {
                    profile: "profile-a".into(),
                    instruction: "hello".into(),
                })
                .unwrap();
            let _ = wait_for_turn(&mut adapter, &id);
            id
        };

        let mut restarted =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        restarted.resume(&id).unwrap();
        restarted.send(&id, "after restart").unwrap();
        let (updates, status) = wait_for_turn(&mut restarted, &id);
        assert_eq!(status, TerminalStatus::Completed);
        assert!(updates.contains(&SessionUpdate::MessageChunk("reply: after restart".into())));

        let invocations: Vec<Vec<String>> = fs::read_to_string(f.record_dir.join("argv.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let started_provider_id = invocations[0]
            .windows(2)
            .find(|pair| pair[0] == "--session-id")
            .unwrap()[1]
            .clone();
        let resumed_provider_id = invocations[1]
            .windows(2)
            .find(|pair| pair[0] == "--resume")
            .unwrap()[1]
            .clone();
        assert_eq!(resumed_provider_id, started_provider_id);
        assert_ne!(resumed_provider_id, id.as_str());
    }

    #[test]
    fn adapter_passes_the_shared_contract_suite() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();

        crate::manager::contract::run_contract_suite(&mut adapter);
    }

    #[test]
    fn unsupported_inspect_fails_with_the_typed_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();

        assert!(unsupported_capability(&adapter.inspect(&id).unwrap_err()).is_some());
    }

    fn wait_for_terminal_status(
        adapter: &mut ClaudeManagerSession,
        id: &GahSessionId,
    ) -> TerminalStatus {
        for _ in 0..300 {
            let _ = adapter.stream(id);
            if let Ok(Some(status)) = adapter.terminal_status(id) {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for a terminal status");
    }

    #[test]
    fn capabilities_advertise_real_interrupt_support() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        assert!(adapter.capabilities().interrupt);
    }

    #[test]
    fn eof_with_outstanding_work_fails_and_kills_a_live_child() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let alive = f.record_dir.join("eof-child-alive.marker");
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "close-output".into(),
            })
            .unwrap();
        for _ in 0..100 {
            if alive.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            alive.exists(),
            "fake child must still be alive after closing stdout"
        );

        let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
            panic!("expected EOF with outstanding work to fail the session");
        };
        assert!(message.contains("closed its structured output"));
        assert!(adapter
            .send(&id, "must not reach the killed child")
            .is_err());
    }

    #[test]
    fn failed_mapping_persistence_reports_cleanup_failure_and_kills_child() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let session_dir = f.record_dir.join("not-a-directory");
        fs::write(&session_dir, "occupied").unwrap();
        let sentinel = f.record_dir.join("slow-completed.marker");
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        adapter.next_cleanup_error = Some("injected surviving descendant".into());

        let error = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("creating Claude session map"));
        assert!(message.contains("Claude process cleanup also failed"));
        assert!(message.contains("injected surviving descendant"));
        assert!(adapter.sessions.is_empty());
        std::thread::sleep(Duration::from_millis(3_200));
        assert!(
            !sentinel.exists(),
            "persistence failure must kill the child"
        );
    }

    #[test]
    fn interrupt_kills_the_child_process_and_sticks_terminated_interrupted() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let sentinel = f.record_dir.join("slow-completed.marker");
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "slow".into(),
            })
            .unwrap();

        // Give the fake backend a moment to actually enter its sleep before
        // interrupting, so this proves a real in-progress turn was killed
        // rather than racing a turn that never started.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(adapter.terminal_status(&id).unwrap(), None);

        adapter.interrupt(&id).unwrap();
        assert_eq!(
            adapter.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Interrupted)
        );

        // The fake backend only writes this sentinel after its multi-second
        // sleep completes. If interrupt merely flipped a status flag without
        // killing the process, the sentinel would still appear once that
        // sleep elapses.
        std::thread::sleep(Duration::from_millis(3_200));
        assert!(
            !sentinel.exists(),
            "interrupt must terminate the real child process, not just relabel status"
        );
    }

    #[test]
    fn queued_turn_failure_remains_sticky_after_a_later_successful_turn() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "delayed-fail".into(),
            })
            .unwrap();
        // Steer/queue a second turn before the first turn's failure has
        // necessarily been observed -- this must not let the second turn's
        // later success overwrite the first turn's failure.
        adapter.send(&id, "should still be sticky failed").unwrap();

        let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
            panic!("expected the queued turn's failure to remain the sticky terminal status");
        };
        assert!(message.contains("structured protocol failure"));

        // Draining further updates and re-polling must not flip this back
        // to Completed once the second (successful) turn's result arrives.
        for _ in 0..20 {
            let _ = adapter.stream(&id);
            assert_eq!(
                adapter.terminal_status(&id).unwrap(),
                Some(TerminalStatus::Failed(message.clone()))
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn explicit_send_after_observed_failure_starts_a_new_working_lifecycle() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "fail".into(),
            })
            .unwrap();
        assert!(matches!(
            wait_for_terminal_status(&mut adapter, &id),
            TerminalStatus::Failed(_)
        ));

        adapter.send(&id, "recovered").unwrap();
        assert_eq!(adapter.terminal_status(&id).unwrap(), None);
        let (updates, status) = wait_for_turn(&mut adapter, &id);
        assert_eq!(status, TerminalStatus::Completed);
        assert!(updates.contains(&SessionUpdate::MessageChunk("reply: recovered".into())));
    }

    #[test]
    fn malformed_protocol_line_terminates_the_child_and_stays_sticky_failed() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "malformed".into(),
            })
            .unwrap();

        let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
            panic!("expected the malformed protocol line to fail the session");
        };
        assert!(message.contains("parsing Claude stream-json event"));

        // A well-formed success result was queued right behind the
        // malformed line on the wire; it must never be allowed to surface,
        // whether because the reader stopped forwarding after the bad line
        // or because a sticky-failed session ignores later terminal events.
        for _ in 0..20 {
            let _ = adapter.stream(&id);
            assert_eq!(
                adapter.terminal_status(&id).unwrap(),
                Some(TerminalStatus::Failed(message.clone())),
                "must never observe the trailing well-formed success result"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn mismatched_session_event_terminates_the_child_and_stays_failed() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_streaming_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "mismatched-session".into(),
            })
            .unwrap();
        adapter
            .sessions
            .get_mut(&id)
            .unwrap()
            .process
            .as_mut()
            .unwrap()
            .injected_cleanup_error = Some("injected mismatch survivor".into());

        let returned = (0..300)
            .find_map(|_| match adapter.stream(&id) {
                Err(error) => Some(error.to_string()),
                Ok(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    None
                }
            })
            .expect("expected the mismatched session event to fail the session");
        let Some(TerminalStatus::Failed(message)) = adapter.terminal_status(&id).unwrap() else {
            panic!("expected the combined failure to be stored");
        };
        assert!(message.contains("returned session ID"));
        assert!(message.contains("injected mismatch survivor"));
        assert_eq!(message, returned);
        assert!(adapter.send(&id, "must not reach a dead child").is_err());
        assert_eq!(
            adapter.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Failed(message))
        );
    }

    #[test]
    fn installed_claude_start_and_restart_resume_smoke_when_requested() {
        let Some(executable) = std::env::var_os("GAH_TEST_REAL_CLAUDE") else {
            return;
        };
        let _exec_guard = crate::test_support::ExecGuard::new();
        let state = tempfile::TempDir::new().unwrap();
        let session_dir = state.path().join("manager-sessions");
        let smoke_args = vec![
            OsString::from("--safe-mode"),
            OsString::from("--tools"),
            OsString::new(),
            OsString::from("--max-budget-usd"),
            OsString::from("0.02"),
        ];
        let id = {
            let mut adapter = ClaudeManagerSession::new_with_session_dir_and_args(
                &executable,
                &session_dir,
                smoke_args.clone(),
            )
            .unwrap();
            let id = adapter
                .start(StartRequest {
                    profile: "installed-smoke".into(),
                    instruction: "Reply with exactly GAH_CLAUDE_START_OK".into(),
                })
                .unwrap();
            assert_bounded_real_turn(&mut adapter, &id, "GAH_CLAUDE_START_OK");
            id
        };

        let mut restarted = ClaudeManagerSession::new_with_session_dir_and_args(
            executable,
            session_dir,
            smoke_args,
        )
        .unwrap();
        restarted.resume(&id).unwrap();
        restarted
            .send(&id, "Reply with exactly GAH_CLAUDE_RESUME_OK")
            .unwrap();
        assert_bounded_real_turn(&mut restarted, &id, "GAH_CLAUDE_RESUME_OK");
    }

    fn assert_bounded_real_turn(
        adapter: &mut ClaudeManagerSession,
        id: &GahSessionId,
        expected_reply: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut updates = Vec::new();
        while Instant::now() < deadline {
            updates.extend(adapter.stream(id).unwrap());
            if let Some(status) = adapter.terminal_status(id).unwrap() {
                updates.extend(adapter.stream(id).unwrap());
                assert_eq!(status, TerminalStatus::Completed);
                let reply = updates
                    .iter()
                    .filter_map(|update| match update {
                        SessionUpdate::MessageChunk(text) => Some(text.as_str()),
                        SessionUpdate::Usage { .. } => None,
                    })
                    .collect::<String>();
                assert_eq!(reply.trim(), expected_reply);
                assert!(
                    updates
                        .iter()
                        .any(|update| matches!(update, SessionUpdate::Usage { .. })),
                    "installed Claude success omitted structured usage"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("installed Claude protocol smoke exceeded its 30-second bound");
    }
}
