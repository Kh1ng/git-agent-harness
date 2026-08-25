//! Provider-neutral manager session protocol (issue #815, split from #520).
//!
//! `src/notifications.rs::spawn_manager_wake` is today's only "talk to a
//! manager CLI" mechanism, and it is exactly one verb: spawn a one-shot
//! process with a single instruction string and never speak to it again --
//! no session identity is captured, nothing can resume it, steer it with a
//! follow-up, stream its output back, interrupt it, or query its status.
//! `ManagerSession` is the real superset that replaces it once real
//! provider adapters (#816 Hermes, #817 Codex, #818 Claude) land; this
//! ticket ships only the trait, a stable session-ID type, and a fully
//! in-memory fake so the contract suite (`contract::run_contract_suite`)
//! has something deterministic to run against before any real adapter
//! exists.
//!
//! Method naming intentionally lines up with the Agent Client Protocol
//! (`apps/server/src/managerChat/acpAdapter.ts`, the TS implementation this
//! will eventually let #834 retire): `start`/`resume`/`send`/`stream`
//! mirror ACP's `newSession`/(reuse)/`prompt`/`sessionUpdate`. ACP itself
//! doesn't exercise an explicit resume or interrupt call in that adapter
//! today, and per-provider model/capability absence is already a normal,
//! non-error outcome there -- both are exactly why `SessionCapabilities`
//! and the typed `UnsupportedCapability` error exist here: a provider
//! declares what it can't do instead of silently no-op-ing.

use anyhow::{Context, Result};
use std::fmt;
use std::str::FromStr;

// `contract` is test-only code (see its own doc comment), but not gated
// under `mod tests` itself: a future real adapter's own `#[cfg(test)] mod
// tests` (e.g. `manager::hermes::tests`) needs to reach
// `super::contract::run_contract_suite` too, and both compile together
// under the same `cfg(test)` build exactly like `crate::test_support` does
// for the rest of this crate.
#[cfg(test)]
mod contract;
pub mod fake;
pub mod hermes;
#[cfg(test)]
mod tests;

/// Stable GAH-owned session identity, independent of any provider's own
/// conversation-ID scheme -- adapters map this to whatever ID the
/// underlying CLI/protocol actually uses (ACP's `sessionId`, a Codex
/// `conversation_id`, etc.), never the other way around. Namespaced like
/// the existing manager-chat memory-gateway key
/// (`apps/server/src/managerChat/memoryGatewayClient.ts`'s
/// `gah:manager:{project}`) so both stay recognizable as the same family
/// of identifier even though this is one session, not a whole project's
/// memory scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GahSessionId(String);

impl GahSessionId {
    pub fn new(profile: &str) -> Self {
        Self(format!("gah:manager:{profile}:{}", uuid::Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GahSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for GahSessionId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let suffix = value
            .strip_prefix("gah:manager:")
            .ok_or_else(|| anyhow::anyhow!("invalid GAH manager session ID"))?;
        let (profile, id) = suffix
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid GAH manager session ID"))?;
        if profile.is_empty() {
            anyhow::bail!("invalid GAH manager session ID");
        }
        uuid::Uuid::parse_str(id).context("invalid GAH manager session UUID")?;
        Ok(Self(value.to_string()))
    }
}

/// What's needed to start a new manager session. Deliberately minimal --
/// only what the fake adapter (and every real adapter) can act on without
/// guessing; extend as a real adapter's `start` needs more.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub profile: String,
    pub instruction: String,
}

/// One normalized unit of streamed output. Message chunks are user-visible
/// assistant text; usage carries the provider's structured context pressure.
/// Tool calls, plans, thoughts, and user-message echoes stay provider-local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    MessageChunk(String),
    Usage { used: u64, size: u64 },
}

/// A session's current lifecycle state, as returned by `inspect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Working,
    Terminated(TerminalStatus),
}

/// How a session ended. Distinct from `SessionStatus::Terminated`'s payload
/// so `terminal_status` can be asked independently of a full `inspect`
/// (some providers may support one without the other).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStatus {
    Completed,
    Failed(String),
    Interrupted,
}

/// Which optional verbs a session adapter actually supports. `start`,
/// `send`, `stream`, and `terminal_status` are the required baseline lifecycle
/// every adapter must implement; `resume`/`interrupt`/`inspect` are the ones
/// real providers are already known to support inconsistently (see the ACP
/// notes above), so a caller can check before calling instead of only
/// discovering it via an `UnsupportedCapability` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCapabilities {
    pub resume: bool,
    pub interrupt: bool,
    pub inspect: bool,
}

/// Typed refusal for a capability a session adapter has declared it does
/// not support, so a caller can distinguish "this provider can't do that"
/// from a genuine runtime failure instead of getting a generic error either
/// way. Mirrors this codebase's established typed-error-through-anyhow
/// convention (e.g. `dispatch::repair_context::StaleSourceError`).
#[derive(Debug)]
pub struct UnsupportedCapability {
    pub capability: &'static str,
}

impl fmt::Display for UnsupportedCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "capability '{}' is not supported by this manager session adapter",
            self.capability
        )
    }
}

impl std::error::Error for UnsupportedCapability {}

pub fn unsupported_capability(err: &anyhow::Error) -> Option<&UnsupportedCapability> {
    err.downcast_ref::<UnsupportedCapability>()
}

/// Provider-neutral manager session protocol. A real adapter wraps exactly
/// one provider CLI/protocol (Hermes ACP, Codex app-server, Claude's resume
/// surface, ...); `fake::FakeManagerSession` wraps none of them and is the
/// suite's deterministic stand-in until #816/#817/#818 land.
///
/// Object-safe by construction (no generic methods) so
/// `contract::run_contract_suite` can run unchanged against `&mut dyn
/// ManagerSession` for the fake today and any real adapter later.
pub trait ManagerSession {
    /// Which optional verbs this adapter supports. Never changes for the
    /// lifetime of the adapter instance -- it reflects what the underlying
    /// provider can do, not any particular session's state.
    fn capabilities(&self) -> SessionCapabilities;

    /// Start a new session and return its stable GAH-owned ID.
    fn start(&mut self, request: StartRequest) -> Result<GahSessionId>;

    /// Reconnect to an existing session so a later `send`/`stream` can
    /// continue it. Returns `Err` carrying `UnsupportedCapability` if
    /// `capabilities().resume` is false.
    fn resume(&mut self, session: &GahSessionId) -> Result<()>;

    /// Send a message into a session -- the initial instruction's follow-up,
    /// or a steer mid-turn. Every adapter must support this.
    fn send(&mut self, session: &GahSessionId, message: &str) -> Result<()>;

    /// Drain whatever output has accumulated since the last `stream` call
    /// (or since `start`, for the first call). Poll-based rather than a
    /// push callback so the trait stays synchronous and object-safe; a real
    /// adapter backed by an async/event-driven protocol buffers internally
    /// between calls.
    fn stream(&mut self, session: &GahSessionId) -> Result<Vec<SessionUpdate>>;

    /// Cancel an in-progress turn. Returns `Err` carrying
    /// `UnsupportedCapability` if `capabilities().interrupt` is false.
    fn interrupt(&mut self, session: &GahSessionId) -> Result<()>;

    /// Query a session's current lifecycle state. Returns `Err` carrying
    /// `UnsupportedCapability` if `capabilities().inspect` is false.
    fn inspect(&mut self, session: &GahSessionId) -> Result<SessionStatus>;

    /// `Some` once the session has ended (however it ended), `None` while
    /// still active. Every adapter must support this even without
    /// `inspect`, since callers need a cheap way to know when to stop
    /// polling `stream`.
    fn terminal_status(&mut self, session: &GahSessionId) -> Result<Option<TerminalStatus>>;
}
