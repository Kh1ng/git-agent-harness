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
    file.sync_all().ok();
    fs::rename(&temp, &path)
        .with_context(|| format!("committing Claude session map {}", path.display()))?;
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
    process: ClaudeProcess,
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
        })
    }

    fn spawn_process(
        &self,
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
        })
    }

    fn terminate_process(process: &mut ClaudeProcess) {
        let _ = crate::runner::process::kill_process_group(&mut process.child);
        let _ = process.child.wait();
    }

    fn pump(&mut self, session: &GahSessionId) -> Result<()> {
        let state = self
            .sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("Claude session {session} must be resumed before use"))?;
        let mut disconnected = false;
        loop {
            match state.process.messages.try_recv() {
                Ok(Ok(message)) => handle_message(state, message)?,
                Ok(Err(error)) => {
                    state.status = SessionStatus::Terminated(TerminalStatus::Failed(error.clone()));
                    return Err(anyhow!(error));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected && state.outstanding_turns > 0 {
            let exit = state.process.child.try_wait()?;
            if let Some(status) = exit {
                let message =
                    format!("Claude Code exited with {status} before a structured result event");
                state.status = SessionStatus::Terminated(TerminalStatus::Failed(message.clone()));
                return Err(anyhow!(message));
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
        if sender.send(message).is_err() {
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
    state.status = if state.outstanding_turns == 0 {
        SessionStatus::Terminated(terminal)
    } else {
        SessionStatus::Working
    };
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
            interrupt: false,
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
                process,
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
                .write_user_message(&provider_session_id, &request.instruction)
                .context("sending Claude's initial structured user message")?;
            persist_mapping(&self.session_dir, &session, &provider_session_id, &cwd)
        })();
        if let Err(error) = started {
            if let Some(mut state) = self.sessions.remove(&session) {
                Self::terminate_process(&mut state.process);
            }
            return Err(error);
        }
        Ok(session)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        let mapping = load_mapping(&self.session_dir, session)?;
        if let Some(mut old) = self.sessions.remove(session) {
            Self::terminate_process(&mut old.process);
        }
        let process = self.spawn_process(&mapping.provider_session_id, &mapping.cwd, true)?;
        self.sessions.insert(
            session.clone(),
            ClaudeSessionState {
                provider_session_id: mapping.provider_session_id,
                process,
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

    fn interrupt(&mut self, _session: &GahSessionId) -> Result<()> {
        Err(UnsupportedCapability {
            capability: "interrupt",
        }
        .into())
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
            Self::terminate_process(&mut state.process);
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
import sys

record_path = {record_path:?}
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
    if prompt == "fail":
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
    fn unsupported_inspect_and_interrupt_fail_with_the_typed_error() {
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

        assert!(unsupported_capability(&adapter.interrupt(&id).unwrap_err()).is_some());
        assert!(unsupported_capability(&adapter.inspect(&id).unwrap_err()).is_some());
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
