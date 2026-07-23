use super::Blocker;
use crate::config::{GahConfig, Profile};
use crate::controller::HumanRequiredReason;
use crate::ledger::{self, LedgerEntry};
use crate::sync;

pub(super) fn policy_approval_still_required(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    entries: &[LedgerEntry],
    work_id: &str,
    gate: &ledger::EffectiveHumanGate,
) -> bool {
    if gate.reason_code.as_deref() != Some(HumanRequiredReason::PolicyApproval.as_str()) {
        return true;
    }
    // TICKET-711: only a review-derived policy_approval gate (raised during
    // review escalation or its post-review repair) is superseded by a stale
    // contract version. A genuine non-review paid-route hold (a regular
    // fix/improve dispatch still awaiting `gah route-approval grant`) must
    // not be silently released just because it predates the contract bump.
    let review_derived =
        gate.mode == "review" || gate.dispatch_reason.as_deref() == Some("post_review_repair");
    if review_derived
        && gate.review_contract_version.unwrap_or(0)
            < crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
    {
        return false;
    }
    let mode = match gate.mode.as_str() {
        "review" => "review",
        "pm" => "pm",
        "experiment" => "experiment",
        _ => "fix",
    };
    let mut current = LedgerEntry::new(profile_name, profile, "auto", mode, work_id, None, None);
    current.work_id = Some(work_id.to_string());
    let runtime = crate::dispatch::routing_runtime_state_from_entries(entries, &current);
    let route_is_eligible = |backend: &str, model: Option<&str>| {
        // Review escalation is ordered and one-shot per backend/model for an
        // unchanged source. A recovered reviewer that already produced the
        // verdict which led to this paid-route gate is not a real escape
        // route: dispatch would only write skipped_duplicate_review and ask
        // for the same paid approval again on the next tick.
        if mode == "review"
            && review_route_was_already_used(
                entries,
                profile_name,
                &profile.repo_id,
                work_id,
                backend,
                model,
                gate.review_generation.as_deref(),
            )
        {
            return false;
        }
        let request = crate::routing::RouteRequest {
            mode,
            requested_backend: backend,
            requested_model: model,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
            last_failure_class: None,
            exact_route_required: false,
        };
        crate::routing::decide_with_state(&cfg.defaults, profile, request, &runtime)
            .ok()
            .is_some_and(|decision| {
                mode != "review"
                    || !review_route_was_already_used(
                        entries,
                        profile_name,
                        &profile.repo_id,
                        work_id,
                        &decision.effective_backend,
                        decision.effective_model.as_deref(),
                        gate.review_generation.as_deref(),
                    )
            })
    };

    // New route failures preserve the exact candidates that were considered.
    // Re-probe those identities explicitly so implementation task rules are
    // honored even though status does not re-fetch the source issue's task
    // metadata. Attempted capability failures remain excluded by `runtime`.
    if let Some(diagnostics) = gate
        .routing_diagnostics
        .as_ref()
        .filter(|diagnostics| !diagnostics.candidates.is_empty())
    {
        return !diagnostics
            .candidates
            .iter()
            .any(|candidate| route_is_eligible(&candidate.backend, candidate.model.as_deref()));
    }

    // Legacy records predate structured route-error diagnostics. Reviews and
    // PM tasks have one mode-level candidate list, so an auto-route probe is
    // exact for those modes. Implementation records fail closed because a
    // task-specific rule may differ from the generic candidate list.
    if matches!(mode, "review" | "pm") {
        return !route_is_eligible("auto", None);
    }
    true
}

fn review_route_was_already_used(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
    backend: &str,
    model: Option<&str>,
    review_generation: Option<&str>,
) -> bool {
    let aliases = ledger::work_id_aliases(work_id);
    let reset_index = entries.iter().rposition(|entry| {
        entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.mode == "clear_attempts"
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
    });
    let active = reset_index.map_or(entries, |index| &entries[index + 1..]);
    active.iter().any(|entry| {
        entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.mode == "review"
            && entry.review_contract_version == Some(ledger::CURRENT_REVIEW_CONTRACT_VERSION)
            && entry.review_generation.as_deref() == review_generation
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
            && entry.effective_backend == backend
            && entry.effective_model.as_deref() == model
            && entry.review_verdict.is_some()
            && entry.failure_class.is_none()
    })
}

fn mr_work_id_from_ledger<'a>(
    mr: &'a sync::SyncMrJson,
    entries: &'a [LedgerEntry],
    profile_name: &str,
    repo_id: &str,
) -> Option<&'a str> {
    mr.work_id.as_deref().or_else(|| {
        entries.iter().rev().find_map(|entry| {
            (entry.profile == profile_name
                && entry.repo_id == repo_id
                && entry.work_id.is_some()
                && (entry.branch.as_deref() == Some(mr.branch.as_str())
                    || (mr.url.is_some() && entry.mr_url.as_deref() == mr.url.as_deref())))
            .then_some(entry.work_id.as_deref())
            .flatten()
        })
    })
}

pub(super) fn project_effective_mr_gates(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    entries: &[LedgerEntry],
    merge_requests: &[sync::SyncMrJson],
    blocked_work_items: &mut Vec<Blocker>,
) {
    for mr in merge_requests {
        let Some(work_id) = mr_work_id_from_ledger(mr, entries, profile_name, &profile.repo_id)
        else {
            continue;
        };
        let Some(gate) = ledger::effective_human_gate_from_entries(
            entries,
            profile_name,
            &profile.repo_id,
            work_id,
        ) else {
            continue;
        };
        let review_derived = gate.mode == "review"
            || (gate.reason_code.as_deref() == Some("stuck_loop_gate")
                && gate.review_generation.is_some());
        if review_derived
            && (gate.review_contract_version != Some(ledger::REVIEW_CONTRACT_VERSION)
                || gate.review_generation.as_deref() != mr.review_generation.as_deref())
        {
            continue;
        }
        if !policy_approval_still_required(cfg, profile_name, profile, entries, work_id, &gate) {
            continue;
        }
        let reason_code = gate.reason_code.clone();
        if let Some(blocker) = blocked_work_items.iter_mut().find(|blocker| {
            blocker.kind == "human_required" && blocker.source_reference.as_deref() == Some(work_id)
        }) {
            // Ticket discovery can project the same gate first, but it only
            // carries a generic message. Enrich that row with the exact MR
            // gate details so the dashboard and notifications explain what
            // is blocked and how to release it without adding a duplicate.
            blocker.reason = reason_code
                .clone()
                .or_else(|| gate.dispatch_reason.clone())
                .or_else(|| blocker.reason.clone());
            if let Some(message) = gate.message.clone() {
                blocker.message = Some(message);
            }
            blocker.reason_code = reason_code;
            continue;
        }
        blocked_work_items.push(Blocker {
            kind: "human_required".into(),
            reason: reason_code
                .clone()
                .or_else(|| gate.dispatch_reason.clone())
                .or(Some("ledger_human_required".into())),
            message: gate
                .message
                .clone()
                .or(Some("Ledger indicates human intervention required".into())),
            backend: None,
            model: None,
            until: None,
            source_reference: Some(work_id.to_string()),
            reason_code,
            remediation_plan: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::{AvailabilityRecord, AvailabilityState, Reason, Source, Status};
    use std::fs;
    use tempfile::TempDir;

    fn make_test_cfg(tmp: &TempDir) -> GahConfig {
        let path = tmp.path().join("cfg.toml");
        fs::write(
            &path,
            r#"
[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
        )
        .unwrap();
        let mut cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        cfg.defaults.artifact_root = tmp.path().to_string_lossy().into_owned();
        cfg
    }

    fn test_mr(work_id: Option<&str>, branch: &str) -> sync::SyncMrJson {
        sync::SyncMrJson {
            profile: Some("test".into()),
            branch: branch.into(),
            work_id: work_id.map(str::to_string),
            id: Some("1".into()),
            url: Some("https://tracker.example/mr/1".into()),
            state: Some("open".into()),
            draft: true,
            merge_status: None,
            merged: false,
            ci_passed: true,
            title: Some("test MR".into()),
            merged_at: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            review_gate_reason: None,
            source_sha: None,
            review_contract_version: ledger::REVIEW_CONTRACT_VERSION,
            review_generation: None,
            review_generation_status: None,
            ci_pending: false,
            classification: "NEEDS_REVIEW".into(),
            recommended_action: sync::RecommendedAction::RunReview,
        }
    }

    #[test]
    fn open_mr_gate_is_projected_without_a_dispatchable_source_ticket_for_both_providers() {
        for provider in ["github", "gitlab"] {
            let tmp = TempDir::new().unwrap();
            let mut cfg = make_test_cfg(&tmp);
            cfg.profiles.get_mut("test").unwrap().provider = provider.into();
            let profile = &cfg.profiles["test"];
            let mut gate = LedgerEntry::new("test", profile, "auto", "fix", "#639", None, None);
            gate.work_id = Some("#639".into());
            gate.branch = Some("gah/fix-639".into());
            gate.human_required = true;
            gate.human_required_reason_code = Some("stuck_loop_gate".into());
            gate.dispatch_reason = Some("stuck_loop_gate".into());

            let mut blockers = Vec::new();
            project_effective_mr_gates(
                &cfg,
                "test",
                profile,
                &[gate],
                &[test_mr(None, "gah/fix-639")],
                &mut blockers,
            );

            assert_eq!(blockers.len(), 1, "provider={provider}");
            assert_eq!(blockers[0].source_reference.as_deref(), Some("#639"));
            assert_eq!(blockers[0].reason_code.as_deref(), Some("stuck_loop_gate"));
        }
    }

    #[test]
    fn mr_gate_projection_is_idempotent_when_ticket_projection_already_added_it() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_test_cfg(&tmp);
        let profile = &cfg.profiles["test"];
        let mut gate = LedgerEntry::new("test", profile, "auto", "fix", "#639", None, None);
        gate.work_id = Some("#639".into());
        gate.human_required = true;
        gate.human_required_reason_code = Some("stuck_loop_gate".into());
        gate.error_summary = Some("exact durable gate detail".into());
        let mut blockers = vec![Blocker {
            kind: "human_required".into(),
            reason: Some("stuck_loop_gate".into()),
            message: None,
            backend: None,
            model: None,
            until: None,
            source_reference: Some("#639".into()),
            reason_code: Some("stuck_loop_gate".into()),
            remediation_plan: None,
        }];

        project_effective_mr_gates(
            &cfg,
            "test",
            profile,
            &[gate],
            &[test_mr(Some("#639"), "gah/fix-639")],
            &mut blockers,
        );

        assert_eq!(blockers.len(), 1);
        assert_eq!(
            blockers[0].message.as_deref(),
            Some("exact durable gate detail")
        );
    }

    #[test]
    fn paid_policy_gate_releases_when_a_subscription_route_recovers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cfg.toml");
        fs::write(
            &path,
            format!(
                r#"
[defaults]
artifact_root = "{}"

[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "{}"
default_target_branch = "main"
claude_path = "/bin/true"
opencode_path = "/bin/true"

[[profiles.test.routing.review_candidates]]
backend = "claude"
model = "sonnet"
priority = 0
included_in_quota = true

[[profiles.test.routing.review_candidates]]
backend = "opencode"
model = "nous/glm-5.2"
priority = 10
requires_approval = true
"#,
                tmp.path().display(),
                tmp.path().display()
            ),
        )
        .unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        let profile = &cfg.profiles["test"];
        let gate = ledger::EffectiveHumanGate {
            reason_code: Some("policy_approval".into()),
            dispatch_reason: Some("review".into()),
            message: None,
            mode: "review".into(),
            timestamp: "2026-07-16T00:00:00Z".into(),
            routing_diagnostics: None,
            review_contract_version: Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION),
            review_generation: None,
        };
        let availability_path = tmp.path().join("availability.json");
        let _availability_guard =
            crate::test_support::AvailabilityEnvGuard::set(&availability_path);

        assert!(!policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[],
            "#650",
            &gate,
        ));

        let mut used_claude =
            LedgerEntry::new("test", profile, "claude", "review", "#650", None, None);
        used_claude.work_id = Some("#650".into());
        used_claude.effective_backend = "claude".into();
        used_claude.effective_model = Some("sonnet".into());
        used_claude.review_verdict = Some("HUMAN_REVIEW".into());
        used_claude.review_contract_version = Some(ledger::REVIEW_CONTRACT_VERSION);
        let unavailable = AvailabilityState {
            version: 1,
            records: vec![AvailabilityRecord {
                backend: "claude".into(),
                backend_instance: None,
                model: Some("sonnet".into()),
                quota_pool: None,
                status: Status::Unavailable,
                reason: Reason::QuotaExhausted,
                observed_at: "2026-07-16T00:00:00Z".into(),
                unavailable_until: Some("2099-01-01T00:00:00Z".into()),
                source: Source::BackendError,
                last_error_summary: None,
            }],
        };
        fs::write(
            &availability_path,
            serde_json::to_string(&unavailable).unwrap(),
        )
        .unwrap();

        assert!(policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[],
            "#650",
            &gate,
        ));

        let wrong_grant = LedgerEntry::new_paid_route_approval(
            "test",
            profile,
            "#650",
            "opencode",
            Some("nous/other-model"),
            true,
        );
        assert!(policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[used_claude.clone(), wrong_grant],
            "#650",
            &gate,
        ));

        let exact_grant = LedgerEntry::new_paid_route_approval(
            "test",
            profile,
            "#650",
            "opencode",
            Some("nous/glm-5.2"),
            true,
        );
        assert!(!policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[used_claude.clone(), exact_grant],
            "#650",
            &gate,
        ));

        fs::write(
            &availability_path,
            serde_json::to_string(&AvailabilityState {
                version: 1,
                records: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        let mut policy_entry =
            LedgerEntry::new("test", profile, "auto", "review", "#650", None, None);
        policy_entry.work_id = Some("#650".into());
        policy_entry.human_required = true;
        policy_entry.human_required_reason_code = Some("policy_approval".into());
        policy_entry.review_contract_version = Some(ledger::REVIEW_CONTRACT_VERSION);
        policy_entry.set_failure(
            crate::ledger::FailureClass::HumanBlocked,
            crate::ledger::FailureStage::Route,
        );
        policy_entry.routing_diagnostics = Some(crate::ledger::RoutingDiagnostics {
            candidates: vec![
                crate::ledger::RoutingCandidateDiagnostic {
                    backend: "opencode".into(),
                    model: Some("nous/glm-5.2".into()),
                    skip_reason: Some("operator_approval_required".into()),
                    ..Default::default()
                },
                crate::ledger::RoutingCandidateDiagnostic {
                    backend: "claude".into(),
                    model: Some("sonnet".into()),
                    skip_reason: Some("model-specific quota_exhausted".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let mut duplicate = LedgerEntry::new("test", profile, "auto", "review", "#650", None, None);
        duplicate.work_id = Some("#650".into());
        duplicate.validation_result = Some("skipped_duplicate_review".into());
        duplicate.review_source_sha = Some("same-sha".into());
        duplicate.reviewer_class = Some("escalatory:claude/sonnet".into());
        duplicate.review_contract_version = Some(ledger::REVIEW_CONTRACT_VERSION);
        let entries = vec![used_claude, policy_entry, duplicate];
        let mut blockers = Vec::new();
        project_effective_mr_gates(
            &cfg,
            "test",
            profile,
            &entries,
            &[test_mr(Some("#650"), "gah/review-650")],
            &mut blockers,
        );
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].reason_code.as_deref(), Some("policy_approval"));
    }

    #[test]
    fn structured_implementation_gate_rechecks_exact_task_route_and_respects_prior_failures() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cfg.toml");
        fs::write(
            &path,
            format!(
                r#"
[defaults]
artifact_root = "{}"

[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "{}"
default_target_branch = "main"
claude_path = "/bin/true"
opencode_path = "/bin/true"

[[profiles.test.routing.improve_candidates]]
backend = "opencode"
model = "nous/glm-5.2"
priority = 10
requires_approval = true

[[profiles.test.routing.task_routing_rules]]
difficulties = ["easy"]

[[profiles.test.routing.task_routing_rules.candidates]]
backend = "claude"
model = "sonnet"
priority = 0
included_in_quota = true
"#,
                tmp.path().display(),
                tmp.path().display()
            ),
        )
        .unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        let profile = &cfg.profiles["test"];
        let availability_path = tmp.path().join("availability.json");
        let _availability_guard =
            crate::test_support::AvailabilityEnvGuard::set(&availability_path);
        let gate = ledger::EffectiveHumanGate {
            reason_code: Some("policy_approval".into()),
            dispatch_reason: Some("initial".into()),
            message: None,
            mode: "fix".into(),
            timestamp: "2026-07-16T00:00:00Z".into(),
            routing_diagnostics: Some(crate::ledger::RoutingDiagnostics {
                candidates: vec![crate::ledger::RoutingCandidateDiagnostic {
                    backend: "claude".into(),
                    model: Some("sonnet".into()),
                    skip_reason: Some("model-specific quota_exhausted".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            review_contract_version: None,
            review_generation: None,
        };

        assert!(!policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[],
            "#650",
            &gate,
        ));

        let mut failed = LedgerEntry::new("test", profile, "claude", "fix", "#650", None, None);
        failed.work_id = Some("#650".into());
        failed.effective_backend = "claude".into();
        failed.effective_model = Some("sonnet".into());
        failed.failure_class = Some("agent_failure".into());
        assert!(policy_approval_still_required(
            &cfg,
            "test",
            profile,
            &[failed],
            "#650",
            &gate,
        ));
    }

    #[test]
    fn pre_bump_review_derived_policy_approval_gate_is_superseded() {
        let (cfg, profile) = policy_approval_test_cfg();
        let gate = ledger::EffectiveHumanGate {
            reason_code: Some("policy_approval".into()),
            dispatch_reason: Some("post_review_repair".into()),
            message: None,
            mode: "review".into(),
            timestamp: "2026-07-16T00:00:00Z".into(),
            review_contract_version: None, // Pre-bump
            review_generation: None,
            routing_diagnostics: None,
        };

        assert!(!policy_approval_still_required(
            &cfg,
            "test",
            &profile,
            &[],
            "#640",
            &gate,
        ));
    }

    #[test]
    fn pre_bump_non_review_policy_approval_gate_still_required() {
        // TICKET-711 regression: a genuine implementation-task paid-route
        // hold (mode "fix", not raised during review or post-review repair)
        // predates the contract bump too, but it is not review-derived and
        // must remain blocked until an actual `gah route-approval grant` is
        // recorded -- not silently released just because it has no stamped
        // review_contract_version.
        let (cfg, profile) = policy_approval_test_cfg();
        let gate = ledger::EffectiveHumanGate {
            reason_code: Some("policy_approval".into()),
            dispatch_reason: Some("initial".into()),
            message: None,
            mode: "fix".into(),
            timestamp: "2026-07-16T00:00:00Z".into(),
            review_contract_version: None, // Pre-bump
            review_generation: None,
            routing_diagnostics: None,
        };

        assert!(policy_approval_still_required(
            &cfg,
            "test",
            &profile,
            &[],
            "#640",
            &gate,
        ));
    }

    fn policy_approval_test_cfg() -> (crate::config::GahConfig, crate::config::Profile) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cfg.toml");
        fs::write(
            &path,
            format!(
                r#"
[defaults]
artifact_root = "{}"

[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "{}"
default_target_branch = "main"
"#,
                tmp.path().display(),
                tmp.path().display()
            ),
        )
        .unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        let profile = cfg.profiles["test"].clone();
        (cfg, profile)
    }
}
