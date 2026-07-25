//! Deterministic remediation planning for blocked work.
//!
//! The planner is intentionally pure: it maps the typed blocker/reason
//! metadata already present in controller and status state to bounded
//! operator actions. It never mutates provider state and never auto-applies
//! approval, review, merge, or retry-budget changes.

use crate::config::Profile;
use crate::controller::HumanRequiredReason;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemediationAuthority {
    Operator,
    PaidRouteApprover,
    HumanReviewer,
    MergeApprover,
    ProfileMaintainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemediationActionKind {
    Command,
    ApiAction,
    ManualReview,
    ManualMerge,
    ConfigChange,
    Inspect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemediationAction {
    pub kind: RemediationActionKind,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_action: Option<String>,
}

impl RemediationAction {
    fn command(summary: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::Command,
            summary: summary.into(),
            command: Some(command.into()),
            api_action: None,
        }
    }

    fn api_action(summary: impl Into<String>, api_action: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::ApiAction,
            summary: summary.into(),
            command: None,
            api_action: Some(api_action.into()),
        }
    }

    fn manual_review(summary: impl Into<String>, api_action: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::ManualReview,
            summary: summary.into(),
            command: None,
            api_action: Some(api_action.into()),
        }
    }

    fn manual_merge(summary: impl Into<String>, api_action: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::ManualMerge,
            summary: summary.into(),
            command: None,
            api_action: Some(api_action.into()),
        }
    }

    fn config_change(summary: impl Into<String>, api_action: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::ConfigChange,
            summary: summary.into(),
            command: None,
            api_action: Some(api_action.into()),
        }
    }

    fn inspect(summary: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            kind: RemediationActionKind::Inspect,
            summary: summary.into(),
            command: Some(command.into()),
            api_action: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RemediationPlan {
    Plan {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        work_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        reason_code: HumanRequiredReason,
        required_authority: RemediationAuthority,
        safe_actions: Vec<RemediationAction>,
    },
    NoAutomaticRemediation {
        profile: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        work_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        reason_code: HumanRequiredReason,
        required_authority: RemediationAuthority,
        safe_actions: Vec<RemediationAction>,
        reason: String,
    },
}

impl RemediationPlan {
    pub fn is_no_automatic_remediation(&self) -> bool {
        matches!(self, Self::NoAutomaticRemediation { .. })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RemediationContext<'a> {
    pub profile_name: &'a str,
    pub profile: &'a Profile,
    pub work_id: Option<&'a str>,
    pub reference: Option<&'a str>,
    pub reason_code: HumanRequiredReason,
    pub blocker_kind: Option<&'a str>,
    pub backend: Option<&'a str>,
    pub model: Option<&'a str>,
}

fn work_label(work_id: Option<&str>) -> String {
    work_id
        .map(str::to_string)
        .unwrap_or_else(|| "<WORK_ID>".to_string())
}

fn inspect_actions(
    profile_name: &str,
    work_id: Option<&str>,
    reference: Option<&str>,
) -> Vec<RemediationAction> {
    let mut actions = vec![RemediationAction::inspect(
        "Inspect the current status snapshot",
        format!("gah status --profile {profile_name} --json"),
    )];
    if let Some(work_id) = work_id {
        actions.push(RemediationAction::inspect(
            "Inspect the blocked work item history",
            format!("gah ledger work {work_id}"),
        ));
    } else if reference.is_some() {
        actions.push(RemediationAction::inspect(
            "Inspect controller activity for the blocked reference",
            format!("gah events --profile {profile_name} --since 7d"),
        ));
    }
    actions
}

fn route_approval_actions(profile_name: &str, work_id: Option<&str>) -> Vec<RemediationAction> {
    vec![
        RemediationAction::command(
            "Grant the exact paid route approval once the operator has validated the route",
            format!(
                "gah route-approval grant --profile {profile_name} {} --backend <backend> --model <model>",
                work_label(work_id)
            ),
        ),
        RemediationAction::inspect(
            "Inspect the blocked work item and route provenance before approving",
            format!("gah ledger work {}", work_label(work_id)),
        ),
    ]
}

fn retry_budget_actions(profile_name: &str, work_id: Option<&str>) -> Vec<RemediationAction> {
    vec![
        RemediationAction::command(
            "Reset the bounded retry ledger for this work item",
            format!(
                "gah ledger clear-attempts --profile {profile_name} {}",
                work_label(work_id)
            ),
        ),
        RemediationAction::inspect(
            "Inspect the retry history before resetting the budget",
            format!("gah ledger work {}", work_label(work_id)),
        ),
    ]
}

fn review_handoff_actions(profile_name: &str, work_id: Option<&str>) -> Vec<RemediationAction> {
    vec![
        RemediationAction::command(
            "Put the work item under a human review hold while the evidence is inspected",
            format!(
                "gah hold set --profile {profile_name} {} --reason \"review evidence handoff\"",
                work_label(work_id)
            ),
        ),
        RemediationAction::api_action(
            "Inspect the review evidence in the provider UI/API",
            "Use the provider UI/API to inspect the review evidence and current verdict before releasing the work item",
        ),
        RemediationAction::manual_review(
            "Review the evidence and record a human decision",
            "Inspect the review evidence in the provider UI/API and the GAH ledger before releasing the work item",
        ),
    ]
}

fn merge_policy_actions(profile_name: &str, work_id: Option<&str>) -> Vec<RemediationAction> {
    let mut actions = vec![RemediationAction::manual_merge(
        "Merge the MR/PR manually in the provider UI/API once the human reviewer is satisfied",
        "Perform the merge outside GAH because the configured merge policy forbids automatic merge",
    )];
    actions.extend(inspect_actions(profile_name, work_id, None));
    actions
}

fn publishing_restriction_actions(
    profile_name: &str,
    work_id: Option<&str>,
) -> Vec<RemediationAction> {
    let mut actions = vec![
        RemediationAction::manual_merge(
            "Merge the existing MR/PR manually in the provider UI/API",
            "GAH will not auto-create or auto-merge this profile's work while publishing is disabled",
        ),
        RemediationAction::config_change(
            "If autonomous publication is intended, enable PR/MR creation for this profile",
            format!(
                "Edit the profile configuration for '{profile_name}' to set publishing.allow_pull_request_creation = true"
            ),
        ),
    ];
    actions.extend(inspect_actions(profile_name, work_id, None));
    actions
}

fn availability_actions(
    profile_name: &str,
    backend: Option<&str>,
    model: Option<&str>,
) -> Vec<RemediationAction> {
    if let Some(backend) = backend {
        let mut command = format!("gah availability clear --backend {backend}");
        if let Some(model) = model {
            command.push_str(&format!(" --model {model}"));
        }
        return vec![
            RemediationAction::command(
                "Clear the stale backend availability record once the backend is healthy again",
                command,
            ),
            RemediationAction::inspect(
                "Validate the profile and routing state before clearing availability",
                format!("gah doctor --profile {profile_name} --validate"),
            ),
        ];
    }

    vec![RemediationAction::inspect(
        "Inspect the profile and routing state before changing availability",
        format!("gah doctor --profile {profile_name} --validate"),
    )]
}

fn plan_with_authority(
    profile_name: &str,
    work_id: Option<&str>,
    reference: Option<&str>,
    reason_code: HumanRequiredReason,
    required_authority: RemediationAuthority,
    safe_actions: Vec<RemediationAction>,
) -> RemediationPlan {
    RemediationPlan::Plan {
        profile: profile_name.to_string(),
        work_id: work_id.map(str::to_string),
        reference: reference.map(str::to_string),
        reason_code,
        required_authority,
        safe_actions,
    }
}

fn no_auto(
    profile_name: &str,
    work_id: Option<&str>,
    reference: Option<&str>,
    reason_code: HumanRequiredReason,
    required_authority: RemediationAuthority,
    reason: impl Into<String>,
    safe_actions: Vec<RemediationAction>,
) -> RemediationPlan {
    RemediationPlan::NoAutomaticRemediation {
        profile: profile_name.to_string(),
        work_id: work_id.map(str::to_string),
        reference: reference.map(str::to_string),
        reason_code,
        required_authority,
        safe_actions,
        reason: reason.into(),
    }
}

pub fn plan_remediation(context: RemediationContext<'_>) -> RemediationPlan {
    let reason_code = context.reason_code;
    let profile_name = context.profile_name;
    let work_id = context.work_id;
    let reference = context.reference;
    match reason_code {
        HumanRequiredReason::PolicyApproval => plan_with_authority(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::PaidRouteApprover,
            route_approval_actions(profile_name, work_id),
        ),
        HumanRequiredReason::RetryBudgetExhausted
        | HumanRequiredReason::FixRetryCapExceeded
        | HumanRequiredReason::MergeRetryCapExceeded => plan_with_authority(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::Operator,
            retry_budget_actions(profile_name, work_id),
        ),
        HumanRequiredReason::ReviewEvidenceGate
        | HumanRequiredReason::ReviewOutputInvalidExhausted
        | HumanRequiredReason::ReviewCeilingExhausted => no_auto(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::HumanReviewer,
            "The blocked item requires a human review decision; no bounded automatic remediation is safe",
            review_handoff_actions(profile_name, work_id),
        ),
        HumanRequiredReason::MergePolicy => no_auto(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::MergeApprover,
            "The configured merge policy explicitly forbids automatic merge",
            merge_policy_actions(profile_name, work_id),
        ),
        HumanRequiredReason::PublishingRestriction => no_auto(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::ProfileMaintainer,
            "Publishing is disabled for this profile, so GAH cannot safely publish or merge automatically",
            publishing_restriction_actions(profile_name, work_id),
        ),
        HumanRequiredReason::ConfigurationInfra => {
            if context.blocker_kind == Some("backend_unavailable") {
                plan_with_authority(
                    profile_name,
                    work_id,
                    reference,
                    reason_code,
                    RemediationAuthority::Operator,
                    availability_actions(profile_name, context.backend, context.model),
                )
            } else {
                no_auto(
                    profile_name,
                    work_id,
                    reference,
                    reason_code,
                    RemediationAuthority::Operator,
                    "The blocker is a configuration or infrastructure problem that needs operator inspection before any bounded remediation is safe",
                    inspect_actions(profile_name, work_id, reference),
                )
            }
        }
        HumanRequiredReason::StuckLoopGate => no_auto(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::Operator,
            "The same lifecycle action repeated without an observable state transition; inspect and correct the work item before explicitly releasing its gate",
            inspect_actions(profile_name, work_id, reference),
        ),
        HumanRequiredReason::Unknown => no_auto(
            profile_name,
            work_id,
            reference,
            reason_code,
            RemediationAuthority::Operator,
            "The blocker reason is unknown or ambiguous; inspect the current state before taking any recovery action",
            inspect_actions(profile_name, work_id, reference),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MergePolicy;

    fn profile(provider: &str, merge_policy: MergePolicy, allow_pr: bool) -> Profile {
        let mut profile = crate::ledger::test_util::profile();
        profile.provider = provider.into();
        profile.publishing.allow_pull_request_creation = allow_pr;
        profile.routing.merge_policy = Some(merge_policy);
        profile
    }

    #[test]
    fn route_approval_plan_is_provider_neutral_and_does_not_bypass_review_or_merge_policy() {
        let github = plan_remediation(RemediationContext {
            profile_name: "real",
            profile: &profile("github", MergePolicy::Auto, true),
            work_id: Some("TICKET-42"),
            reference: Some("https://example.invalid/pull/42"),
            reason_code: HumanRequiredReason::PolicyApproval,
            blocker_kind: Some("human_required"),
            backend: None,
            model: None,
        });
        let gitlab = plan_remediation(RemediationContext {
            profile_name: "real",
            profile: &profile("gitlab", MergePolicy::Auto, true),
            work_id: Some("TICKET-42"),
            reference: Some("https://example.invalid/merge_requests/42"),
            reason_code: HumanRequiredReason::PolicyApproval,
            blocker_kind: Some("human_required"),
            backend: None,
            model: None,
        });

        let RemediationPlan::Plan {
            profile,
            work_id,
            reason_code,
            required_authority,
            safe_actions,
            reference,
        } = github
        else {
            panic!("expected actionable route approval plan");
        };
        let RemediationPlan::Plan {
            profile: gitlab_profile,
            work_id: gitlab_work_id,
            reason_code: gitlab_reason_code,
            required_authority: gitlab_required_authority,
            safe_actions: gitlab_safe_actions,
            reference: gitlab_reference,
        } = gitlab
        else {
            panic!("expected actionable route approval plan");
        };
        assert_eq!(profile, gitlab_profile);
        assert_eq!(work_id, gitlab_work_id);
        assert_eq!(reason_code, gitlab_reason_code);
        assert_eq!(required_authority, RemediationAuthority::PaidRouteApprover);
        assert_eq!(
            gitlab_required_authority,
            RemediationAuthority::PaidRouteApprover
        );
        assert_ne!(reference, gitlab_reference);
        assert!(reference
            .as_deref()
            .is_some_and(|reference| reference.contains("/pull/42")));
        assert!(gitlab_reference
            .as_deref()
            .is_some_and(|reference| reference.contains("/merge_requests/42")));
        assert_eq!(safe_actions, gitlab_safe_actions);
        assert!(safe_actions.iter().any(|action| action
            .command
            .as_deref()
            .is_some_and(|cmd| cmd.contains("route-approval grant"))));
        assert!(safe_actions.iter().all(|action| action
            .command
            .as_deref()
            .is_none_or(|cmd| !cmd.contains("merge_policy"))));
    }

    #[test]
    fn human_review_and_merge_policy_remain_no_automatic_remediation() {
        let cases = [
            (
                HumanRequiredReason::ReviewEvidenceGate,
                RemediationAuthority::HumanReviewer,
            ),
            (
                HumanRequiredReason::MergePolicy,
                RemediationAuthority::MergeApprover,
            ),
            (
                HumanRequiredReason::PublishingRestriction,
                RemediationAuthority::ProfileMaintainer,
            ),
        ];

        for (reason_code, expected_authority) in cases {
            let plan = plan_remediation(RemediationContext {
                profile_name: "real",
                profile: &profile("github", MergePolicy::StopForHuman, false),
                work_id: Some("TICKET-42"),
                reference: Some("https://example.invalid/pull/42"),
                reason_code,
                blocker_kind: Some("human_required"),
                backend: None,
                model: None,
            });
            let RemediationPlan::NoAutomaticRemediation {
                required_authority,
                safe_actions,
                ..
            } = plan
            else {
                panic!("expected explicit no-automatic-remediation for {reason_code:?}");
            };
            assert_eq!(required_authority, expected_authority);
            assert!(safe_actions
                .iter()
                .any(|action| action.kind != RemediationActionKind::Command
                    || action.command.is_some()
                    || action.api_action.is_some()));
            assert!(safe_actions.iter().all(|action| action
                .command
                .as_deref()
                .is_none_or(|cmd| !cmd.contains("route-approval grant"))));
            assert!(safe_actions.iter().all(|action| action
                .command
                .as_deref()
                .is_none_or(|cmd| !cmd.contains("clear-attempts"))));
        }
    }

    #[test]
    fn recoverable_configuration_errors_can_emit_safe_clear_commands() {
        let plan = plan_remediation(RemediationContext {
            profile_name: "real",
            profile: &profile("github", MergePolicy::Auto, true),
            work_id: None,
            reference: None,
            reason_code: HumanRequiredReason::ConfigurationInfra,
            blocker_kind: Some("backend_unavailable"),
            backend: Some("codex"),
            model: Some("gpt-5"),
        });

        let RemediationPlan::Plan { safe_actions, .. } = plan else {
            panic!("expected actionable config/infra remediation");
        };
        assert!(safe_actions.iter().any(|action| action
            .command
            .as_deref()
            .is_some_and(|cmd| cmd.contains("availability clear"))));
    }
}
