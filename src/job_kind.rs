//! Canonical dispatch job-kind identity.
//!
//! Before this module, "what kind of job is this" was independently
//! matched as a bare `&str` (`DispatchArgs.mode`/`LedgerEntry.mode`) in
//! ~30 places across `dispatch/*.rs`, `routing/*.rs`, `config.rs`,
//! `ledger/jsonl.rs`, `controller/runtime*.rs`, `status/*.rs`,
//! `telemetry/extractor.rs`, `quota_snapshot.rs`, and `events.rs` -- the
//! same pre-`BackendKind` state backends and providers were in.
//!
//! `mode`/`LedgerEntry.mode` stay raw `String`s at the CLI/JSONL wire
//! boundary (same pattern as `Profile.provider`/`ProviderKind`) -- this is
//! the typed accessor call sites should use instead of re-matching the
//! bare string.
//!
//! `parse` is permissive, folding one legacy value seen in real ledger data
//! down to its current kind: `"implement"` (an old mode value, no longer
//! producible by `dispatch::run`, but still present in historical ledger
//! rows and defensively matched in `work_identity.rs`) folds to `Improve`.
//!
//! `"pm_orchestration"` deliberately does **not** fold to `Pm`, despite
//! looking like the same "legacy alias" shape: it's a genuinely distinct
//! control-plane record type (`controller::runtime::pm::append_failure`'s
//! PM-publish-failure marker), written *alongside* real `mode: "pm"`
//! dispatch entries, not instead of them -- `status/pm.rs` deliberately
//! groups them for one failure-counting purpose (`matches!(mode, "pm" |
//! "pm_orchestration")`) without meaning they're the same job kind
//! everywhere. Same family of adjacent-but-distinct markers as
//! `"pm_publish"`/`"pm_reconcile"`/`"clear_attempts"`/`"claim"` --
//! `LedgerEntry.mode` is a broader "record type" field than `JobKind`
//! covers, and control/audit markers outside the 5 real job kinds are
//! intentionally left as plain string matches wherever they appear mixed
//! in with real job kinds (see `ledger/jsonl.rs::effective_human_gate_for_scope`
//! and `status/pm.rs`, neither converted here for this reason).
//!
//! Do not confuse `JobKind` with `NextAction` (`controller::action`) --
//! that's a different, already-well-typed axis (the controller's automatic
//! next-step decision), not a job kind. Also don't confuse with the
//! separate `dispatch_reason` label strings (`"fix_existing"`,
//! `"dispatch_ticket"`, `"retry"`, `"escalate"`) passed to
//! `run_dispatch_and_record` -- an unrelated audit-log tag, not a mode.
//!
//! `family()` replaces three independently-hand-duplicated copies of the
//! same 3-way grouping that already existed before this module
//! (`routing/policy.rs`, repeated 7+ times, and `config.rs`'s
//! `RoutingPolicy::find_quota_pool`, a fully independent copy of the same
//! logic) -- a real, live duplication-drift risk, not just a string swap.
//!
//! `Research`/`Audit` (added after the initial 5) are both read-only,
//! investigation-only job kinds: no worktree mutation, no commit, no PR --
//! a backend runs, investigates, and self-reports findings as a GitHub
//! issue via its own `gh` CLI access (the same way improve/fix backends
//! self-push branches), rather than GAH orchestrating issue creation in
//! Rust. `Audit` additionally has the backend pull `gh api .../dependabot/alerts`
//! as a first, structured data source before its own dead-code/complexity
//! pass. Both map to `JobFamily::Investigation`, a new family with no
//! dedicated routing config (falls back to `default_backend`/`default_model`,
//! same as the pre-existing "no family-specific config" fallback every
//! `JobFamily` match already had).
//!
//! `research` collided with an unrelated existing string: `pm.rs`'s
//! `RecommendedRouting.capability` (a PM-ticket-decomposition hint, not a
//! job kind) used to accept `"research"` as one of its four values. Renamed
//! to `"investigate"` there instead of compromising this kind's name --
//! see `models.rs::RecommendedRouting` and `dispatch/workflows/pm.rs`.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Improve,
    Fix,
    Pm,
    Review,
    Experiment,
    Research,
    Audit,
}

/// The behavioral grouping routing policy actually cares about: `pm` and
/// `review` are their own thing, `improve`/`fix`/`experiment` share backend/
/// model policy, candidate pools, and load-balancing, and `research`/`audit`
/// (read-only investigation, no dedicated routing config yet) share the
/// same "fall back to defaults" treatment every family match already had
/// for anything outside its three original arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobFamily {
    Pm,
    Review,
    ImproveLike,
    Investigation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownJobKind(pub String);

impl fmt::Display for UnknownJobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown job kind '{}'", self.0)
    }
}

impl std::error::Error for UnknownJobKind {}

impl JobKind {
    pub fn parse(s: &str) -> Result<Self, UnknownJobKind> {
        match s {
            "improve" | "implement" => Ok(Self::Improve),
            "fix" => Ok(Self::Fix),
            "pm" => Ok(Self::Pm),
            "review" => Ok(Self::Review),
            "experiment" => Ok(Self::Experiment),
            "research" => Ok(Self::Research),
            "audit" => Ok(Self::Audit),
            other => Err(UnknownJobKind(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Improve => "improve",
            Self::Fix => "fix",
            Self::Pm => "pm",
            Self::Review => "review",
            Self::Experiment => "experiment",
            Self::Research => "research",
            Self::Audit => "audit",
        }
    }

    pub fn family(&self) -> JobFamily {
        match self {
            Self::Pm => JobFamily::Pm,
            Self::Review => JobFamily::Review,
            Self::Improve | Self::Fix | Self::Experiment => JobFamily::ImproveLike,
            Self::Research | Self::Audit => JobFamily::Investigation,
        }
    }

    pub fn all() -> [JobKind; 7] {
        [
            Self::Improve,
            Self::Fix,
            Self::Pm,
            Self::Review,
            Self::Experiment,
            Self::Research,
            Self::Audit,
        ]
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_canonical_name_round_trips() {
        for kind in JobKind::all() {
            assert_eq!(JobKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn legacy_aliases_fold_to_canonical_kind() {
        assert_eq!(JobKind::parse("implement").unwrap(), JobKind::Improve);
    }

    #[test]
    fn unknown_strings_are_rejected() {
        assert!(JobKind::parse("not-a-real-mode").is_err());
        // dispatch_reason label, not a mode
        assert!(JobKind::parse("dispatch_ticket").is_err());
        assert!(JobKind::parse("retry").is_err());
        assert!(JobKind::parse("escalate").is_err());
        // a genuinely distinct control-plane record type, not a Pm alias --
        // see module docs
        assert!(JobKind::parse("pm_orchestration").is_err());
        assert!(JobKind::parse("pm_publish").is_err());
        assert!(JobKind::parse("pm_reconcile").is_err());
    }

    #[test]
    fn family_groups_improve_fix_experiment_together() {
        assert_eq!(JobKind::Improve.family(), JobFamily::ImproveLike);
        assert_eq!(JobKind::Fix.family(), JobFamily::ImproveLike);
        assert_eq!(JobKind::Experiment.family(), JobFamily::ImproveLike);
        assert_eq!(JobKind::Pm.family(), JobFamily::Pm);
        assert_eq!(JobKind::Review.family(), JobFamily::Review);
    }

    #[test]
    fn family_groups_research_and_audit_together() {
        assert_eq!(JobKind::Research.family(), JobFamily::Investigation);
        assert_eq!(JobKind::Audit.family(), JobFamily::Investigation);
    }

    #[test]
    fn all_returns_exactly_seven_kinds_with_no_duplicates() {
        let all = JobKind::all();
        assert_eq!(all.len(), 7);
        let mut seen = std::collections::HashSet::new();
        for kind in all {
            assert!(seen.insert(kind), "duplicate kind in all(): {kind:?}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        for kind in JobKind::all() {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }
}
