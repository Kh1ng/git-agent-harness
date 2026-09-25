use serde::{Deserialize, Serialize};

/// Provider conversation that performed the work. Notifications use this to
/// resume the same agent; the configured manager remains the fallback.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionBackend {
    Claude,
    Codex,
    #[serde(other)]
    Unknown,
}

impl AgentSessionBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for AgentSessionBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AgentSessionRef {
    pub backend: AgentSessionBackend,
    pub provider_session_id: String,
    /// Exact checkout used by the attempt. Resumption is refused if it no longer exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// Resolved backend executable, including an instance-specific binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Per-attempt provider state root. This restores the conversation store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_values_remain_readable() {
        assert_eq!(
            serde_json::from_str::<AgentSessionBackend>(r#""future-agent""#).unwrap(),
            AgentSessionBackend::Unknown
        );
        assert_eq!(
            serde_json::to_string(&AgentSessionBackend::Codex).unwrap(),
            r#""codex""#
        );
        let legacy: AgentSessionRef = serde_json::from_str(
            r#"{"backend":"codex","provider_session_id":"session","working_directory":"/tmp/work"}"#,
        )
        .unwrap();
        assert_eq!(legacy.executable, None);
        assert_eq!(legacy.home, None);
    }
}
