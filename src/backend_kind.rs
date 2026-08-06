//! Canonical backend-kind identity.
//!
//! Before this module, "the list of all GAH backends" was independently
//! hardcoded as `&str` matches in at least 7 places:
//! `execution_identity::runner_kind_for_backend`,
//! `runner::resolve::backend_command_name`, `config::Profile::
//! configured_backend_path`, a model-pin validation match in `config.rs`,
//! `config::backend_instances`'s instance validation, `dispatch::attempts`'s
//! dispatch match, and independent per-backend matches in `runner::review`
//! and `runner::review_usage`. None of those were compiler-enforced to stay
//! in sync (all `&str` matches with a wildcard fallthrough), and at least
//! one pair had already drifted (`runner::review`'s argv construction vs.
//! the worker path it was supposed to mirror).
//!
//! `BackendKind` is the app, full stop -- `parse` recognizes canonical
//! command names plus genuine rebrand aliases for the *same* app
//! (`cloud-coder`/`auto` -> `Openhands`, an old name for one app, not a
//! second app). It deliberately does **not** recognize `agy-main`/
//! `agy-second`: those name *instances* of the Agy app (two accounts
//! sharing one CLI), which is a config/`backend_instance` concern, not a
//! kind. A handful of legacy call sites still accept those two strings
//! directly as backend identifiers (pre-dating the `backend_instances`
//! config mechanism) and fold them to `Agy` locally, each with a comment
//! pointing at the tracking issue to retire the string convention in favor
//! of real `backend_instances` entries -- that folding intentionally does
//! not live here, so this type stays exactly "the app" as designed.
//!
//! Hermes is included: manager chat already dispatches it as a real coding
//! backend (`apps/server/src/managerChat/`), so it belongs in the same
//! vocabulary as the other six even though `runner::backends` has no
//! `hermes.rs` yet and dispatch for it isn't wired up (see call sites that
//! match exhaustively over `BackendKind` for the explicit "not yet
//! implemented" arm).
//!
//! Deliberately not covered here: `notifications.rs`'s `current_manager`
//! match, which selects the CLI to notify on autonomous wake events -- a
//! different concept (manager-wake target, not dispatch backend) that just
//! happens to also include `"hermes"`.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Claude,
    Codex,
    Opencode,
    Openhands,
    Vibe,
    Agy,
    Hermes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownBackendKind(pub String);

impl fmt::Display for UnknownBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown backend kind '{}'", self.0)
    }
}

impl std::error::Error for UnknownBackendKind {}

impl BackendKind {
    /// Folds every alias already recognized somewhere in the codebase down
    /// to one of the six canonical kinds. See module docs for exactly what
    /// this preserves vs. what it deliberately leaves untouched.
    pub fn parse(s: &str) -> Result<Self, UnknownBackendKind> {
        match s {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            "openhands" | "cloud-coder" | "auto" => Ok(Self::Openhands),
            "vibe" => Ok(Self::Vibe),
            "agy" => Ok(Self::Agy),
            "hermes" => Ok(Self::Hermes),
            other => Err(UnknownBackendKind(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Openhands => "openhands",
            Self::Vibe => "vibe",
            Self::Agy => "agy",
            Self::Hermes => "hermes",
        }
    }

    pub fn all() -> [BackendKind; 7] {
        [
            Self::Claude,
            Self::Codex,
            Self::Opencode,
            Self::Openhands,
            Self::Vibe,
            Self::Agy,
            Self::Hermes,
        ]
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_canonical_name_round_trips() {
        for kind in BackendKind::all() {
            assert_eq!(BackendKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn known_aliases_fold_to_canonical_kind() {
        assert_eq!(
            BackendKind::parse("cloud-coder").unwrap(),
            BackendKind::Openhands
        );
        assert_eq!(BackendKind::parse("auto").unwrap(), BackendKind::Openhands);
    }

    #[test]
    fn instance_names_are_not_kinds() {
        // agy-main/agy-second name *instances* of the Agy app, not separate
        // kinds -- that's a backend_instance/config concern. Callers that
        // still accept these two strings directly fold them to Agy locally,
        // not through this parser (see resolve.rs::backend_command_name).
        assert!(BackendKind::parse("agy-main").is_err());
        assert!(BackendKind::parse("agy-second").is_err());
    }

    #[test]
    fn unknown_strings_are_rejected() {
        assert!(BackendKind::parse("not-a-real-backend").is_err());
        // manager-wake target (notifications.rs) -- a different concept
        // that also happens to include "claude"/"codex"/"hermes", but
        // "hermes" itself IS a valid BackendKind (see module docs).
        assert!(BackendKind::parse("hermes").is_ok());
        // test-only logical-backend name used in execution_identity.rs's
        // tests to prove quota-pool scoping stays distinct -- never a real
        // runner kind
        assert!(BackendKind::parse("opencode-alt").is_err());
    }

    #[test]
    fn all_returns_exactly_seven_kinds_with_no_duplicates() {
        let all = BackendKind::all();
        assert_eq!(all.len(), 7);
        let mut seen = std::collections::HashSet::new();
        for kind in all {
            assert!(seen.insert(kind), "duplicate kind in all(): {kind:?}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        for kind in BackendKind::all() {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }
}
