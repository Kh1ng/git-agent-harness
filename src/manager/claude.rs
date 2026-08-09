//! Claude Code-backed `ManagerSession`.
//!
//! Claude already has the session primitive we need here: a stable session
//! UUID, `--resume` for continuing an existing conversation, and structured
//! JSON output for each turn. This adapter keeps the GAH-owned session id
//! namespaced while reusing the UUID suffix as Claude's own session id.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeAuthState {
    pub logged_in: Option<bool>,
    pub auth_method: Option<String>,
    pub api_provider: Option<String>,
    pub subscription_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDiscovery {
    pub executable: PathBuf,
    pub version: Option<String>,
    pub auth: ClaudeAuthState,
}

#[derive(Debug)]
struct ClaudeSessionState {
    provider_session_id: String,
    status: SessionStatus,
    pending: Vec<SessionUpdate>,
}

#[derive(Debug)]
pub struct ClaudeManagerSession {
    pub discovery: ClaudeDiscovery,
    sessions: HashMap<GahSessionId, ClaudeSessionState>,
}

impl ClaudeManagerSession {
    pub fn new() -> Result<Self> {
        let executable = crate::runner::resolve::resolve_executable_on_path("claude")
            .context("claude executable not found on PATH")?;
        Self::with_executable(executable)
    }

    pub fn with_executable(executable: impl Into<PathBuf>) -> Result<Self> {
        let executable = executable.into();
        if !crate::runner::is_executable_path(&executable) {
            anyhow::bail!(
                "configured executable '{}' does not exist or is not executable",
                executable.display()
            );
        }
        let version = discover_version(&executable);
        let auth = discover_auth_state(&executable);
        Ok(Self {
            discovery: ClaudeDiscovery {
                executable,
                version,
                auth,
            },
            sessions: HashMap::new(),
        })
    }

    pub fn discovery(&self) -> &ClaudeDiscovery {
        &self.discovery
    }

    fn provider_session_id(session: &GahSessionId) -> Result<String> {
        let raw = session
            .as_str()
            .rsplit_once(':')
            .map(|(_, suffix)| suffix)
            .ok_or_else(|| anyhow!("invalid Claude session id '{}'", session))?;
        uuid::Uuid::parse_str(raw)
            .with_context(|| format!("invalid Claude session UUID suffix '{raw}'"))?;
        Ok(raw.to_string())
    }

    fn session_mut(&mut self, session: &GahSessionId) -> Result<&mut ClaudeSessionState> {
        self.sessions
            .get_mut(session)
            .ok_or_else(|| anyhow!("unknown Claude session '{}'", session))
    }

    fn turn_json(
        &self,
        provider_session_id: &str,
        prompt: &str,
        resume: bool,
    ) -> Result<ClaudeTurn> {
        let mut cmd = Command::new(&self.discovery.executable);
        cmd.arg("--print").arg("--output-format").arg("json");
        if resume {
            cmd.arg("--resume").arg(provider_session_id);
        } else {
            cmd.arg("--session-id").arg(provider_session_id);
        }
        cmd.arg(prompt);

        let output = cmd
            .output()
            .with_context(|| "launching claude; is it installed and on PATH?")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = parse_turn_result(&stdout).with_context(|| {
            format!("claude turn did not return structured JSON for session {provider_session_id}")
        })?;
        if let Some(returned_session_id) = parsed.session_id.as_deref() {
            if returned_session_id != provider_session_id {
                return Err(anyhow!(
                    "claude returned session id '{}' for request '{}'",
                    returned_session_id,
                    provider_session_id
                ));
            }
        }
        if !output.status.success() || parsed.is_error {
            return Err(anyhow!(
                "{}",
                parsed.error_message.unwrap_or_else(|| format!(
                    "claude turn failed for session {provider_session_id}"
                ))
            ));
        }
        Ok(parsed)
    }

    fn append_turn(&mut self, session_id: &GahSessionId, turn: ClaudeTurn) -> Result<()> {
        let state = self.session_mut(session_id)?;
        state.pending.push(SessionUpdate::MessageChunk(turn.text));
        Ok(())
    }
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
        let session_id = GahSessionId::new(&request.profile);
        let provider_session_id = Self::provider_session_id(&session_id)?;
        let turn = self.turn_json(&provider_session_id, &request.instruction, false)?;
        self.sessions.insert(
            session_id.clone(),
            ClaudeSessionState {
                provider_session_id,
                status: SessionStatus::Working,
                pending: vec![SessionUpdate::MessageChunk(turn.text)],
            },
        );
        Ok(session_id)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities().resume {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        let provider_session_id = Self::provider_session_id(session)?;
        self.sessions
            .entry(session.clone())
            .or_insert_with(|| ClaudeSessionState {
                provider_session_id,
                status: SessionStatus::Working,
                pending: Vec::new(),
            });
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let provider_session_id = {
            let state = self.session_mut(session)?;
            state.provider_session_id.clone()
        };
        let turn = self.turn_json(&provider_session_id, message, true)?;
        self.append_turn(session, turn)
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        let state = self.session_mut(session)?;
        Ok(std::mem::take(&mut state.pending))
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
        let state = self.session_mut(session)?;
        Ok(match &state.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}

#[derive(Debug)]
struct ClaudeTurn {
    text: String,
    is_error: bool,
    error_message: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeTurnJson {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    api_error_status: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

fn parse_turn_result(stdout: &str) -> Result<ClaudeTurn> {
    let parsed: ClaudeTurnJson = serde_json::from_str(stdout.trim())
        .context("expected a single structured JSON result from claude")?;
    let text = parsed.result.unwrap_or_else(|| {
        parsed
            .stop_reason
            .unwrap_or_else(|| "Claude completed without a textual result".to_string())
    });
    let error_message = parsed.error.as_ref().and_then(|value| {
        value.as_str().map(str::to_string).or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    });
    Ok(ClaudeTurn {
        text: if text.trim().is_empty() {
            "Claude completed without a textual result".to_string()
        } else {
            text
        },
        is_error: parsed.is_error,
        error_message: error_message.or(parsed.api_error_status),
        session_id: parsed.session_id,
    })
}

#[derive(Debug, Deserialize)]
struct ClaudeAuthStatusRaw {
    #[serde(default)]
    #[serde(rename = "loggedIn")]
    logged_in: Option<bool>,
    #[serde(default)]
    #[serde(rename = "authMethod")]
    auth_method: Option<String>,
    #[serde(default)]
    #[serde(rename = "apiProvider")]
    api_provider: Option<String>,
    #[serde(default)]
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

fn discover_auth_state(executable: &Path) -> ClaudeAuthState {
    let output = Command::new(executable).args(["auth", "status"]).output();
    let Ok(output) = output else {
        return ClaudeAuthState::default();
    };
    let parsed = serde_json::from_slice::<ClaudeAuthStatusRaw>(&output.stdout).ok();
    parsed.map_or_else(ClaudeAuthState::default, |raw| ClaudeAuthState {
        logged_in: raw.logged_in,
        auth_method: raw.auth_method,
        api_provider: raw.api_provider,
        subscription_type: raw.subscription_type,
    })
}

fn discover_version(executable: &Path) -> Option<String> {
    let output = Command::new(executable).arg("--version").output().ok()?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::contract::run_contract_suite;
    use crate::runner::backends::test_util::{fixture, make_fake_bin};
    use crate::test_support::ExecGuard;
    use std::fs;

    fn write_fake_claude(dir: &Path, record_dir: &Path) {
        let body = format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"--version\" ]; then\n\
  echo '2.1.197 (Claude Code)'\n\
  exit 0\n\
fi\n\
if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then\n\
  printf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"subscriptionType\":\"pro\"}}'\n\
  exit 0\n\
fi\n\
mode=unknown\n\
session_id=\n\
prompt=\n\
while [ $# -gt 0 ]; do\n\
  case \"$1\" in\n\
    --print|--verbose)\n\
      shift\n\
      ;;\n\
    --output-format)\n\
      shift 2\n\
      ;;\n\
    --session-id)\n\
      mode=start\n\
      session_id=$2\n\
      shift 2\n\
      ;;\n\
    --resume)\n\
      mode=resume\n\
      session_id=$2\n\
      shift 2\n\
      ;;\n\
    *)\n\
      prompt=$*\n\
      break\n\
      ;;\n\
  esac\n\
done\n\
printf '%s\\n' \"$mode\" > '{mode}'\n\
printf '%s\\n' \"$session_id\" > '{session_id}'\n\
printf '%s\\n' \"$prompt\" > '{prompt}'\n\
printf '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ack: %s\",\"stop_reason\":\"end_turn\",\"session_id\":\"%s\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":2}},\"modelUsage\":{{}},\"permission_denials\":[],\"terminal_reason\":\"completed\"}}\\n' \"$prompt\" \"$session_id\"\n",
            mode = record_dir.join("mode.txt").display(),
            session_id = record_dir.join("session-id.txt").display(),
            prompt = record_dir.join("prompt.txt").display(),
        );
        make_fake_bin(dir, "claude", &body);
    }

    #[test]
    fn discovery_records_version_and_auth_state_without_tokens() {
        let _exec_guard = ExecGuard::new();
        let f = fixture();
        write_fake_claude(&f.bin_dir, &f.record_dir);
        let adapter = ClaudeManagerSession::with_executable(f.bin_dir.join("claude")).unwrap();

        assert_eq!(adapter.discovery.executable, f.bin_dir.join("claude"));
        assert_eq!(
            adapter.discovery.version.as_deref(),
            Some("2.1.197 (Claude Code)")
        );
        assert_eq!(adapter.discovery.auth.logged_in, Some(true));
        assert_eq!(
            adapter.discovery.auth.auth_method.as_deref(),
            Some("claude.ai")
        );
        assert_eq!(
            adapter.discovery.auth.api_provider.as_deref(),
            Some("firstParty")
        );
        assert_eq!(
            adapter.discovery.auth.subscription_type.as_deref(),
            Some("pro")
        );
    }

    #[test]
    fn claude_adapter_contract_suite_uses_the_real_resume_surface() {
        let _exec_guard = ExecGuard::new();
        let f = fixture();
        write_fake_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::with_executable(f.bin_dir.join("claude")).unwrap();

        run_contract_suite(&mut adapter);
    }

    #[test]
    fn claude_adapter_resume_uses_the_existing_session_uuid() {
        let _exec_guard = ExecGuard::new();
        let f = fixture();
        write_fake_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::with_executable(f.bin_dir.join("claude")).unwrap();

        let session = adapter
            .start(StartRequest {
                profile: "contract-suite".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        let suffix = session.as_str().rsplit_once(':').unwrap().1.to_string();
        assert_eq!(
            fs::read_to_string(f.record_dir.join("mode.txt"))
                .unwrap()
                .trim(),
            "start"
        );
        assert_eq!(
            fs::read_to_string(f.record_dir.join("session-id.txt"))
                .unwrap()
                .trim(),
            suffix
        );
        assert_eq!(
            adapter.stream(&session).unwrap(),
            vec![SessionUpdate::MessageChunk("ack: hello".into())]
        );

        let mut restarted =
            ClaudeManagerSession::with_executable(f.bin_dir.join("claude")).unwrap();
        restarted.resume(&session).unwrap();
        restarted.send(&session, "follow up").unwrap();
        assert_eq!(
            fs::read_to_string(f.record_dir.join("mode.txt"))
                .unwrap()
                .trim(),
            "resume"
        );
        assert_eq!(
            fs::read_to_string(f.record_dir.join("session-id.txt"))
                .unwrap()
                .trim(),
            suffix
        );
        assert_eq!(
            restarted.stream(&session).unwrap(),
            vec![SessionUpdate::MessageChunk("ack: follow up".into())]
        );
    }

    #[test]
    fn unsupported_capabilities_fail_closed() {
        let _exec_guard = ExecGuard::new();
        let f = fixture();
        write_fake_claude(&f.bin_dir, &f.record_dir);
        let mut adapter = ClaudeManagerSession::with_executable(f.bin_dir.join("claude")).unwrap();
        let session = adapter
            .start(StartRequest {
                profile: "contract-suite".into(),
                instruction: "hello".into(),
            })
            .unwrap();

        let err = adapter.interrupt(&session).unwrap_err();
        assert!(super::super::unsupported_capability(&err).is_some());

        let err = adapter.inspect(&session).unwrap_err();
        assert!(super::super::unsupported_capability(&err).is_some());
    }
}
