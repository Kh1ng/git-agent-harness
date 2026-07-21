//! TICKET-077: durable, typed controller actions. The schema only -- no
//! execution here (see `dispatch::run` for execution, wired from
//! `gah loop`, TICKET-079).
//!
//! Every variant carries a mandatory `reason` (why this action was
//! selected) plus enough identity to execute it without re-observing
//! state. Serializable so it can be persisted verbatim into a controller
//! event (TICKET-083).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum NextAction {
    ReviewMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    MarkReadyForReview {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    FixMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        /// Exact source/metadata generation whose findings authorize this
        /// repair. `None` is retained for backwards-compatible event reads;
        /// newly-decided autonomous repairs always populate it.
        #[serde(default)]
        review_generation: Option<String>,
        reason: String,
    },
    /// TICKET-127: auto-merge -- a strong-tier reviewer's APPROVE (high
    /// confidence) plus conclusively-green CI, gated by the same retry cap
    /// as FixMr.
    MergeMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        /// Exact source/metadata generation approved for merge. Old persisted
        /// actions deserialize as `None` and fail closed during execution.
        #[serde(default)]
        review_generation: Option<String>,
        reason: String,
    },
    DispatchTicket {
        ticket_path: String,
        work_id: Option<String>,
        recommended_backend: Option<String>,
        recommended_model: Option<String>,
        reason: String,
    },
    /// A trusted provider issue explicitly marked for bounded PM
    /// decomposition. Planning and publication execute under one durable
    /// work-item claim, never alongside normal implementation of this source.
    DecomposeIssue {
        ticket_path: String,
        work_id: String,
        title: Option<String>,
        reason: String,
    },
    /// All provider-native children of a published PM plan are terminal.
    /// Recording this does not itself close the source issue.
    ReconcilePmParent {
        work_id: String,
        source_issue_number: String,
        plan_fingerprint: String,
        child_issue_numbers: Vec<String>,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt failed for an
    /// infra reason (harness/environment/backend/unknown) that has since
    /// cleared -- same backend/model as before, not escalated.
    Retry {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt was a genuine
    /// agent-capability failure (agent_no_progress/agent_failure),
    /// requesting a stronger backend/model this time.
    Escalate {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    WaitUntil {
        until: String,
        reason: String,
    },
    HumanRequired {
        reason: String,
        #[serde(default)]
        reference: Option<String>,
        /// TICKET-505: stable reason code for why autonomy stopped.
        #[serde(default)]
        reason_code: Option<String>,
    },
    NoOp {
        reason: String,
    },
}

impl NextAction {
    /// Coarse type name for logging/fingerprinting (TICKET-081) -- stable
    /// even if variant fields change shape.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ReviewMr { .. } => "review_mr",
            Self::MarkReadyForReview { .. } => "mark_ready_for_review",
            Self::FixMr { .. } => "fix_mr",
            Self::MergeMr { .. } => "merge_mr",
            Self::DispatchTicket { .. } => "dispatch_ticket",
            Self::DecomposeIssue { .. } => "decompose_issue",
            Self::ReconcilePmParent { .. } => "reconcile_pm_parent",
            Self::Retry { .. } => "retry",
            Self::Escalate { .. } => "escalate",
            Self::WaitUntil { .. } => "wait_until",
            Self::HumanRequired { .. } => "human_required",
            Self::NoOp { .. } => "no_op",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::ReviewMr { reason, .. }
            | Self::MarkReadyForReview { reason, .. }
            | Self::FixMr { reason, .. }
            | Self::MergeMr { reason, .. }
            | Self::DispatchTicket { reason, .. }
            | Self::DecomposeIssue { reason, .. }
            | Self::ReconcilePmParent { reason, .. }
            | Self::Retry { reason, .. }
            | Self::Escalate { reason, .. }
            | Self::WaitUntil { reason, .. }
            | Self::HumanRequired { reason, .. }
            | Self::NoOp { reason } => reason,
        }
    }

    /// The work_id this action is about, where one exists. Used for
    /// fingerprinting (TICKET-081) and event logging (TICKET-083).
    pub fn work_id(&self) -> Option<&str> {
        match self {
            Self::ReviewMr { work_id, .. }
            | Self::MarkReadyForReview { work_id, .. }
            | Self::FixMr { work_id, .. }
            | Self::MergeMr { work_id, .. } => work_id.as_deref(),
            Self::DispatchTicket { work_id, .. } => work_id.as_deref(),
            Self::DecomposeIssue { work_id, .. }
            | Self::ReconcilePmParent { work_id, .. }
            | Self::Retry { work_id, .. }
            | Self::Escalate { work_id, .. } => Some(work_id),
            Self::WaitUntil { .. } | Self::HumanRequired { .. } | Self::NoOp { .. } => None,
        }
    }

    /// TICKET-505: Returns the stable reason code for HumanRequired actions.
    /// Returns None for non-HumanRequired actions.
    pub fn human_required_reason_code(&self) -> Option<&str> {
        match self {
            Self::HumanRequired { reason_code, .. } => reason_code.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NextAction;

    #[test]
    fn kind_is_stable_short_name_per_variant() {
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert_eq!(action.kind(), "no_op");
        assert_eq!(action.reason(), "nothing actionable");
        assert_eq!(action.work_id(), None);
    }

    #[test]
    fn retry_and_escalate_expose_work_id() {
        let retry = NextAction::Retry {
            work_id: "TICKET-042".into(),
            ticket_path: "docs/tickets/TICKET-042-x.md".into(),
            reason: "infra failure cleared".into(),
        };
        assert_eq!(retry.kind(), "retry");
        assert_eq!(retry.work_id(), Some("TICKET-042"));

        let escalate = NextAction::Escalate {
            work_id: "TICKET-043".into(),
            ticket_path: "docs/tickets/TICKET-043-y.md".into(),
            reason: "no progress last attempt".into(),
        };
        assert_eq!(escalate.kind(), "escalate");
        assert_eq!(escalate.work_id(), Some("TICKET-043"));

        let ready = NextAction::MarkReadyForReview {
            work_id: Some("TICKET-044".into()),
            branch: "gah/real-4".into(),
            mr_url: Some("https://example/pull/4".into()),
            reason: "CI green, still draft".into(),
        };
        assert_eq!(ready.kind(), "mark_ready_for_review");
        assert_eq!(ready.work_id(), Some("TICKET-044"));
    }

    #[test]
    fn round_trips_through_json() {
        let action = NextAction::ReviewMr {
            work_id: Some("TICKET-001".into()),
            branch: "gah/real-1".into(),
            mr_url: Some("https://example/pull/1".into()),
            reason: "classified NEEDS_REVIEW".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn mark_ready_round_trips_through_json() {
        let action = NextAction::MarkReadyForReview {
            work_id: Some("TICKET-004".into()),
            branch: "gah/real-4".into(),
            mr_url: Some("https://example/pull/4".into()),
            reason: "CI green, still draft".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn pm_actions_round_trip_with_durable_identity() {
        let actions = [
            NextAction::DecomposeIssue {
                ticket_path: "#561".into(),
                work_id: "#561".into(),
                title: Some("Large story".into()),
                reason: "planning label".into(),
            },
            NextAction::ReconcilePmParent {
                work_id: "#561".into(),
                source_issue_number: "561".into(),
                plan_fingerprint: "plan-a".into(),
                child_issue_numbers: vec!["600".into()],
                reason: "children terminal".into(),
            },
        ];
        for action in actions {
            let parsed: NextAction =
                serde_json::from_str(&serde_json::to_string(&action).unwrap()).unwrap();
            assert_eq!(parsed, action);
            assert_eq!(parsed.work_id(), Some("#561"));
        }
    }

    #[test]
    fn wait_until_and_human_required_have_no_work_id() {
        let wait = NextAction::WaitUntil {
            until: "2026-07-06T00:00:00Z".into(),
            reason: "backend unavailable".into(),
        };
        assert_eq!(wait.work_id(), None);

        let human = NextAction::HumanRequired {
            reason: "MR ready for human decision".into(),
            reference: Some("https://example/pull/2".into()),
            reason_code: Some("merge_policy".into()),
        };
        assert_eq!(human.work_id(), None);
        assert_eq!(human.human_required_reason_code(), Some("merge_policy"));
    }

    #[test]
    fn human_required_without_reason_code() {
        let human = NextAction::HumanRequired {
            reason: "MR ready for human decision".into(),
            reference: Some("https://example/pull/2".into()),
            reason_code: None,
        };
        assert_eq!(human.human_required_reason_code(), None);
    }
}
