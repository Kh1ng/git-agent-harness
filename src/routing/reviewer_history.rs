use super::{CandidateIdentity, ReviewerOutcomeMetrics};
use crate::job_kind::JobKind;
use crate::ledger::{reconcile::ReconciliationEntry, LedgerEntry};
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const MAX_REVIEW_HISTORY: usize = 200;

pub(crate) fn reviewer_outcome_metrics(
    entries: &[LedgerEntry],
    reconciliations: &[ReconciliationEntry],
    profile: &str,
    repo_id: &str,
) -> HashMap<CandidateIdentity, ReviewerOutcomeMetrics> {
    // ponytail: bounded linear scans are enough for 200 reviews; index by
    // branch only if route planning becomes measurably slow.
    let reviews = entries
        .iter()
        .rev()
        .filter(|entry| {
            entry.profile == profile
                && entry.repo_id == repo_id
                && JobKind::parse(&entry.mode) == Ok(JobKind::Review)
        })
        .filter_map(|entry| review_verdict(entry).map(|verdict| (entry, verdict)))
        .take(MAX_REVIEW_HISTORY)
        .collect::<Vec<_>>();
    let mut metrics: HashMap<CandidateIdentity, ReviewerOutcomeMetrics> = HashMap::new();

    for (review, verdict) in reviews {
        let identity = review_identity(review);
        let outcome = metrics.entry(identity).or_default();
        outcome.completed_reviews += 1;
        if let Some(duration) = review.duration_seconds.filter(|value| value.is_finite()) {
            outcome.latency_total_seconds += duration.max(0.0);
            outcome.latency_samples += 1;
        }
        match review.usage.usage_classification.as_deref() {
            Some("quota_backed" | "subscription") => outcome.quota_backed_reviews += 1,
            Some("api_key_backed") => {
                outcome.api_backed_reviews += 1;
                if let Some(cost) = review
                    .usage
                    .actual_cost_usd
                    .or(review.usage.estimated_cost_usd)
                    .filter(|value| value.is_finite())
                {
                    outcome.api_cost_total_usd += cost.max(0.0);
                    outcome.api_cost_samples += 1;
                }
            }
            _ => {}
        }

        let later_fix = entries.iter().any(|entry| {
            same_work(review, entry)
                && happened_after(&entry.timestamp, &review.timestamp)
                && entry.dispatch_reason.as_deref() == Some("post_review_repair")
        });
        let merged = reconciliations.iter().any(|entry| {
            reconciliation_matches(review, entry)
                && happened_after(&entry.timestamp, &review.timestamp)
                && entry.new_state.eq_ignore_ascii_case("merged")
        });
        if later_fix {
            outcome.later_fix_correlations += 1;
        }

        match verdict {
            "APPROVE" if later_fix => {
                outcome.outcome_samples += 1;
                outcome.false_approvals += 1;
            }
            "APPROVE" if merged => {
                outcome.outcome_samples += 1;
                outcome.successful_outcomes += 1;
            }
            "NEEDS_FIX" | "REJECT" if later_fix => {
                outcome.outcome_samples += 1;
                outcome.successful_outcomes += 1;
            }
            "NEEDS_FIX" | "REJECT" if merged => {
                outcome.outcome_samples += 1;
                outcome.human_overrides += 1;
                outcome.false_rejections += 1;
            }
            _ => {}
        }
    }
    metrics
}

fn review_verdict(entry: &LedgerEntry) -> Option<&str> {
    entry
        .review_verdict
        .as_deref()
        .or(entry.validation_result.as_deref())
        .filter(|verdict| {
            matches!(
                *verdict,
                "APPROVE" | "NEEDS_FIX" | "REJECT" | "HUMAN_REVIEW"
            )
        })
}

fn review_identity(entry: &LedgerEntry) -> CandidateIdentity {
    entry
        .attempt_routing
        .iter()
        .rev()
        .find_map(|attempt| attempt.identity.as_ref())
        .map(CandidateIdentity::from_execution_identity)
        .unwrap_or_else(|| {
            CandidateIdentity::new(
                entry
                    .reviewer_backend
                    .as_deref()
                    .unwrap_or(&entry.effective_backend),
                entry
                    .reviewer_model
                    .as_deref()
                    .or(entry.effective_model.as_deref()),
            )
        })
}

fn same_work(review: &LedgerEntry, candidate: &LedgerEntry) -> bool {
    review.profile == candidate.profile
        && review.repo_id == candidate.repo_id
        && review.branch.is_some()
        && review.branch == candidate.branch
}

fn reconciliation_matches(review: &LedgerEntry, entry: &ReconciliationEntry) -> bool {
    entry
        .profile
        .as_deref()
        .is_some_and(|value| value == review.profile)
        && entry
            .repo_id
            .as_deref()
            .is_some_and(|value| value == review.repo_id)
        && (review.branch.is_some() && review.branch == entry.branch
            || review.mr_url.is_some() && review.mr_url == entry.mr_url
            || review
                .work_id
                .as_deref()
                .is_some_and(|value| value == entry.work_id))
}

fn happened_after(candidate: &str, reference: &str) -> bool {
    match (
        OffsetDateTime::parse(candidate, &Rfc3339),
        OffsetDateTime::parse(reference, &Rfc3339),
    ) {
        (Ok(candidate), Ok(reference)) => candidate > reference,
        _ => candidate > reference,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::test_util::profile;

    fn review(branch: &str, backend: &str, model: &str, verdict: &str) -> LedgerEntry {
        let mut entry = LedgerEntry::new("test", &profile(), backend, "review", branch, None, None);
        entry.repo_id = "repo".into();
        entry.branch = Some(branch.into());
        entry.effective_backend = backend.into();
        entry.effective_model = Some(model.into());
        entry.review_verdict = Some(verdict.into());
        entry.duration_seconds = Some(10.0);
        entry
    }

    #[test]
    fn outcomes_keep_quota_cost_and_human_overrides_distinct() {
        let mut approved = review("branch-a", "claude", "sonnet", "APPROVE");
        approved.timestamp = "2026-01-01T00:00:00Z".into();
        approved.usage.usage_classification = Some("quota_backed".into());
        approved.usage.actual_cost_usd = Some(9.0);
        let mut rejected = review("branch-b", "codex", "gpt", "NEEDS_FIX");
        rejected.timestamp = "2026-01-01T00:00:01Z".into();
        rejected.usage.usage_classification = Some("api_key_backed".into());
        rejected.usage.actual_cost_usd = Some(0.25);
        let mut false_approval = review("branch-c", "codex", "gpt", "APPROVE");
        false_approval.timestamp = "2026-01-01T00:00:02Z".into();
        let mut repair =
            LedgerEntry::new("test", &profile(), "codex", "fix", "branch-c", None, None);
        repair.repo_id = "repo".into();
        repair.branch = Some("branch-c".into());
        repair.timestamp = "2026-01-01T00:00:03Z".into();
        repair.dispatch_reason = Some("post_review_repair".into());
        let merged = |branch: &str, work_id: &str| ReconciliationEntry {
            timestamp: "2026-01-02T00:00:00Z".into(),
            record_type: "mr_state".into(),
            work_id: work_id.into(),
            branch: Some(branch.into()),
            mr_url: None,
            previous_state: None,
            new_state: "MERGED".into(),
            source: "test".into(),
            mr_id: None,
            source_issue_number: None,
            previous_issue_state: None,
            resulting_issue_state: None,
            issue_closure_mode: None,
            issue_closure_classification: None,
            issue_closure_reason: None,
            profile: Some("test".into()),
            repo_id: Some("repo".into()),
        };
        let metrics = reviewer_outcome_metrics(
            &[approved, rejected, false_approval, repair],
            &[merged("branch-a", "#1"), merged("branch-b", "#2")],
            "test",
            "repo",
        );

        let quota = &metrics[&CandidateIdentity::new("claude", Some("sonnet"))];
        assert_eq!(quota.successful_outcomes, 1);
        assert_eq!(quota.quota_backed_reviews, 1);
        assert_eq!(quota.average_api_cost_usd(), None);
        let api = &metrics[&CandidateIdentity::new("codex", Some("gpt"))];
        assert_eq!(api.human_overrides, 1);
        assert_eq!(api.false_rejections, 1);
        assert_eq!(api.false_approvals, 1);
        assert_eq!(api.later_fix_correlations, 1);
        assert_eq!(api.average_api_cost_usd(), Some(0.25));
    }

    #[test]
    fn exact_backend_instances_with_the_same_runner_and_model_stay_separate() {
        let mut first = review("branch-a", "agy", "gemini", "APPROVE");
        let mut second = review("branch-b", "agy", "gemini", "APPROVE");
        for (entry, instance) in [(&mut first, "agy-primary"), (&mut second, "agy-secondary")] {
            let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
                "agy",
                Some("gemini"),
                None::<String>,
            );
            identity.explicit_instance = true;
            identity.backend_instance = instance.into();
            entry
                .attempt_routing
                .push(crate::ledger::AttemptRoutingRecord {
                    attempt_number: 1,
                    backend_instance: instance.into(),
                    effective_model: Some("gemini".into()),
                    identity: Some(identity),
                    routing_diagnostics: None,
                });
        }

        let metrics = reviewer_outcome_metrics(&[first, second], &[], "test", "repo");

        assert_eq!(
            metrics[&CandidateIdentity::new("agy-primary", Some("gemini"))].completed_reviews,
            1
        );
        assert_eq!(
            metrics[&CandidateIdentity::new("agy-secondary", Some("gemini"))].completed_reviews,
            1
        );
    }
}
