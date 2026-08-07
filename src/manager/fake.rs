//! Deterministic in-memory `ManagerSession` -- no real provider process
//! involved. Exists so `contract::run_contract_suite` has something to run
//! against before #816/#817/#818 land, and so the eventual real adapters
//! can be tested against the exact same suite unchanged.

use super::{
    GahSessionId, ManagerSession, SessionCapabilities, SessionStatus, SessionUpdate, StartRequest,
    TerminalStatus, UnsupportedCapability,
};
use anyhow::Result;
use std::collections::HashMap;

struct FakeSession {
    status: SessionStatus,
    pending: Vec<SessionUpdate>,
}

/// `capabilities` is configurable (not hardcoded to "everything on") so the
/// contract suite can also exercise the `UnsupportedCapability` path
/// deterministically -- a real adapter genuinely varies here per provider.
pub struct FakeManagerSession {
    capabilities: SessionCapabilities,
    sessions: HashMap<GahSessionId, FakeSession>,
}

impl FakeManagerSession {
    pub fn new(capabilities: SessionCapabilities) -> Self {
        Self {
            capabilities,
            sessions: HashMap::new(),
        }
    }

    fn session_mut(&mut self, session: &GahSessionId) -> Result<&mut FakeSession> {
        self.sessions
            .get_mut(session)
            .ok_or_else(|| anyhow::anyhow!("unknown fake session {session}"))
    }
}

impl ManagerSession for FakeManagerSession {
    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities
    }

    fn start(&mut self, request: StartRequest) -> Result<GahSessionId> {
        let id = GahSessionId::new(&request.profile);
        self.sessions.insert(
            id.clone(),
            FakeSession {
                status: SessionStatus::Working,
                pending: vec![SessionUpdate::MessageChunk(format!(
                    "ack: {}",
                    request.instruction
                ))],
            },
        );
        Ok(id)
    }

    fn resume(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.resume {
            return Err(UnsupportedCapability {
                capability: "resume",
            }
            .into());
        }
        self.session_mut(session)?;
        Ok(())
    }

    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()> {
        let session = self.session_mut(session)?;
        session
            .pending
            .push(SessionUpdate::MessageChunk(format!("ack: {message}")));
        Ok(())
    }

    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>> {
        let session = self.session_mut(session)?;
        Ok(std::mem::take(&mut session.pending))
    }

    fn interrupt(&mut self, session: &GahSessionId) -> Result<()> {
        if !self.capabilities.interrupt {
            return Err(UnsupportedCapability {
                capability: "interrupt",
            }
            .into());
        }
        let session = self.session_mut(session)?;
        session.status = SessionStatus::Terminated(TerminalStatus::Interrupted);
        Ok(())
    }

    fn inspect(&mut self, session: &GahSessionId) -> Result<SessionStatus> {
        if !self.capabilities.inspect {
            return Err(UnsupportedCapability {
                capability: "inspect",
            }
            .into());
        }
        Ok(self.session_mut(session)?.status.clone())
    }

    fn terminal_status(&mut self, session: &GahSessionId) -> Result<Option<TerminalStatus>> {
        let session = self.session_mut(session)?;
        Ok(match &session.status {
            SessionStatus::Terminated(status) => Some(status.clone()),
            _ => None,
        })
    }
}
