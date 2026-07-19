use super::ActiveClaimSnapshot;
use crate::sync::SyncMrJson;
use std::collections::HashSet;

pub(super) struct IntakeState {
    pub(super) open_mrs: u32,
    pub(super) inflight_implementations: u32,
    pub(super) paused: bool,
}

pub(super) fn project(
    merge_requests: &[SyncMrJson],
    active_claims: &[ActiveClaimSnapshot],
    limit: u32,
) -> IntakeState {
    let open = merge_requests
        .iter()
        .filter(|mr| !matches!(mr.classification.as_str(), "MERGED" | "CLOSED_UNMERGED"))
        .collect::<Vec<_>>();
    let open_work_ids = open
        .iter()
        .filter_map(|mr| mr.work_id.as_deref())
        .collect::<HashSet<_>>();
    let open_mrs = open.len() as u32;
    let inflight_implementations = active_claims
        .iter()
        .filter(|claim| !open_work_ids.contains(claim.work_id.as_str()))
        .count() as u32;
    IntakeState {
        open_mrs,
        inflight_implementations,
        paused: open_mrs.saturating_add(inflight_implementations) >= limit.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(work_id: &str, classification: &str) -> SyncMrJson {
        SyncMrJson {
            profile: None,
            branch: format!("gah/{work_id}"),
            work_id: Some(work_id.into()),
            id: Some(work_id.into()),
            url: None,
            state: Some("OPEN".into()),
            draft: true,
            merge_status: None,
            merged: classification == "MERGED",
            merged_at: None,
            ci_passed: false,
            ci_pending: false,
            title: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            review_gate_reason: None,
            source_sha: None,
            review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
            review_generation: None,
            review_generation_status: None,
            classification: classification.into(),
            recommended_action: crate::sync::RecommendedAction::RunReview,
        }
    }

    fn claim(work_id: &str) -> ActiveClaimSnapshot {
        ActiveClaimSnapshot {
            work_id: work_id.into(),
            pid: 1,
            scope: "test@repo".into(),
            hostname: "test".into(),
            claimed_at: "2026-07-17T00:00:00Z".into(),
            age_seconds: 1,
        }
    }

    #[test]
    fn claims_for_existing_mrs_are_lifecycle_work_not_new_intake() {
        let state = project(
            &[mr("#1", "NEEDS_REVIEW"), mr("#old", "MERGED")],
            &[claim("#1"), claim("#2")],
            2,
        );

        assert_eq!(state.open_mrs, 1);
        assert_eq!(state.inflight_implementations, 1);
        assert!(state.paused);
    }
}
