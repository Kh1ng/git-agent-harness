use super::types::{candidate_label, render_skips, SkippedBackend};
use crate::ledger::{RoutingCandidateDiagnostic, RoutingDiagnostics};
use crate::quota::{self, PaceBand, PacingConfig};

/// Read-only boundary between routing policy internals and diagnostics.
///
/// Candidate construction and ordering deliberately remain outside this
/// module. Future policy modules can implement this trait without exposing
/// their concrete candidate representation to diagnostics.
pub(super) trait DiagnosticCandidate {
    fn backend(&self) -> &str;
    fn model(&self) -> Option<&str>;
    fn quota_pool(&self) -> Option<&str>;
    fn priority(&self) -> i32;
    fn included_in_quota(&self) -> bool;
    fn marginal_cost_usd(&self) -> Option<f64>;
    fn quota_usage_percent(&self) -> Option<f64>;
    fn quota_days_remaining(&self) -> Option<f64>;
    fn requires_approval(&self) -> bool;
    fn original_order(&self) -> usize;
}

pub(super) fn describe_candidate<C: DiagnosticCandidate + ?Sized>(
    candidate: &C,
    pacing: &PacingConfig,
) -> String {
    let mut details = Vec::new();
    if candidate.included_in_quota() {
        details.push(
            match quota::quota_pace(
                candidate.quota_usage_percent(),
                candidate.quota_days_remaining(),
                pacing,
            )
            .unwrap_or(PaceBand::Normal)
            {
                PaceBand::AggressiveBurn => "included quota aggressive-burn".into(),
                PaceBand::MildBurn => "included quota mild-burn".into(),
                PaceBand::Normal => "included quota normal".into(),
                PaceBand::Conserve => "included quota conserve".into(),
                PaceBand::HardConserve => "included quota hard-conserve".into(),
            },
        );
    } else if candidate.requires_approval() {
        details.push("paid approval required".into());
    } else if let Some(cost) = candidate.marginal_cost_usd() {
        details.push(format!("paid ${cost:.4}"));
    }
    if candidate.priority() != 0 {
        details.push(format!("priority {}", candidate.priority()));
    }
    let label = candidate_label(candidate.backend(), candidate.model());
    if details.is_empty() {
        label
    } else {
        format!("{label} ({})", details.join(", "))
    }
}

pub(super) fn build_routing_diagnostics<C: DiagnosticCandidate>(
    candidates: &[C],
    selected: &C,
    skipped: &[SkippedBackend],
    selected_over: Option<&[String]>,
    pacing: &PacingConfig,
) -> RoutingDiagnostics {
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(consideration_order, candidate)| {
            let skipped = skipped.iter().find(|skip| {
                skip.backend == candidate.backend() && skip.model.as_deref() == candidate.model()
            });
            RoutingCandidateDiagnostic {
                backend: candidate.backend().to_string(),
                model: candidate.model().map(str::to_string),
                quota_pool: candidate.quota_pool().map(str::to_string),
                default_order: Some(candidate.original_order()),
                consideration_order: Some(consideration_order),
                pace_band: candidate_pace_band(candidate, pacing),
                cost_class: Some(candidate_cost_class(candidate)),
                skip_reason: skipped.map(|skip| skip.reason.clone()),
                unavailable_until: skipped.and_then(|skip| skip.unavailable_until.clone()),
            }
        })
        .collect();

    RoutingDiagnostics {
        policy_reordered_candidates: selected_over.is_some(),
        selected_backend: Some(selected.backend().to_string()),
        selected_model: selected.model().map(str::to_string),
        selected_quota_pool: selected.quota_pool().map(str::to_string),
        selected_pace_band: candidate_pace_band(selected, pacing),
        selected_cost_class: Some(candidate_cost_class(selected)),
        selected_over: selected_over.unwrap_or_default().to_vec(),
        human_summary: Some(render_routing_diagnostics_human(
            selected,
            skipped,
            selected_over,
            pacing,
        )),
        candidates,
    }
}

fn render_routing_diagnostics_human<C: DiagnosticCandidate + ?Sized>(
    selected: &C,
    skipped: &[SkippedBackend],
    selected_over: Option<&[String]>,
    pacing: &PacingConfig,
) -> String {
    let mut parts = vec![format!("selected {}", describe_candidate(selected, pacing))];
    if let Some(pool) = selected.quota_pool() {
        parts.push(format!("quota pool {pool}"));
    }
    if let Some(band) = candidate_pace_band(selected, pacing) {
        parts.push(format!("pace {band}"));
    }
    parts.push(format!("cost {}", candidate_cost_class(selected)));
    if let Some(selected_over) = selected_over.filter(|items| !items.is_empty()) {
        parts.push(format!(
            "policy reordered defaults over {}",
            selected_over.join(", ")
        ));
    }
    if !skipped.is_empty() {
        parts.push(format!("skipped {}", render_skips(skipped)));
    }
    parts.join("; ")
}

fn candidate_pace_band<C: DiagnosticCandidate + ?Sized>(
    candidate: &C,
    pacing: &PacingConfig,
) -> Option<String> {
    if !candidate.included_in_quota() {
        return None;
    }
    Some(
        match quota::quota_pace(
            candidate.quota_usage_percent(),
            candidate.quota_days_remaining(),
            pacing,
        )
        .unwrap_or(PaceBand::Normal)
        {
            PaceBand::AggressiveBurn => "aggressive_burn",
            PaceBand::MildBurn => "mild_burn",
            PaceBand::Normal => "normal",
            PaceBand::Conserve => "conserve",
            PaceBand::HardConserve => "hard_conserve",
        }
        .to_string(),
    )
}

fn candidate_cost_class<C: DiagnosticCandidate + ?Sized>(candidate: &C) -> String {
    if candidate.included_in_quota() {
        "included_quota".into()
    } else if candidate.requires_approval() || candidate.marginal_cost_usd().unwrap_or(0.0) > 0.0 {
        "paid".into()
    } else {
        "standard".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Candidate {
        backend: &'static str,
        model: Option<&'static str>,
        quota_pool: Option<&'static str>,
        priority: i32,
        included_in_quota: bool,
        marginal_cost_usd: Option<f64>,
        quota_usage_percent: Option<f64>,
        quota_days_remaining: Option<f64>,
        requires_approval: bool,
        original_order: usize,
    }

    impl DiagnosticCandidate for Candidate {
        fn backend(&self) -> &str {
            self.backend
        }
        fn model(&self) -> Option<&str> {
            self.model
        }
        fn quota_pool(&self) -> Option<&str> {
            self.quota_pool
        }
        fn priority(&self) -> i32 {
            self.priority
        }
        fn included_in_quota(&self) -> bool {
            self.included_in_quota
        }
        fn marginal_cost_usd(&self) -> Option<f64> {
            self.marginal_cost_usd
        }
        fn quota_usage_percent(&self) -> Option<f64> {
            self.quota_usage_percent
        }
        fn quota_days_remaining(&self) -> Option<f64> {
            self.quota_days_remaining
        }
        fn requires_approval(&self) -> bool {
            self.requires_approval
        }
        fn original_order(&self) -> usize {
            self.original_order
        }
    }

    fn included_candidate() -> Candidate {
        Candidate {
            backend: "codex",
            model: Some("gpt-5.4-mini"),
            quota_pool: Some("codex-main"),
            priority: 2,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            quota_usage_percent: Some(20.0),
            quota_days_remaining: Some(5.0),
            requires_approval: false,
            original_order: 1,
        }
    }

    #[test]
    fn diagnostics_preserve_structured_fields_and_exact_human_summary() {
        let selected = included_candidate();
        let candidates = vec![selected.clone()];
        let skipped = vec![SkippedBackend {
            backend: "vibe".into(),
            model: Some("devstral-small".into()),
            reason: "quota_exhausted".into(),
            unavailable_until: Some("tomorrow".into()),
        }];
        let selected_over = vec!["vibe/devstral-small".into()];

        let diagnostics = build_routing_diagnostics(
            &candidates,
            &selected,
            &skipped,
            Some(&selected_over),
            &PacingConfig::default(),
        );

        assert!(diagnostics.policy_reordered_candidates);
        assert_eq!(diagnostics.selected_backend.as_deref(), Some("codex"));
        assert_eq!(diagnostics.selected_model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(
            diagnostics.selected_quota_pool.as_deref(),
            Some("codex-main")
        );
        assert_eq!(
            diagnostics.selected_cost_class.as_deref(),
            Some("included_quota")
        );
        assert_eq!(diagnostics.selected_over, selected_over);
        assert_eq!(diagnostics.candidates[0].default_order, Some(1));
        assert_eq!(diagnostics.candidates[0].consideration_order, Some(0));
        assert_eq!(
            diagnostics.human_summary.as_deref(),
            Some("selected codex/gpt-5.4-mini (included quota aggressive-burn, priority 2); quota pool codex-main; pace aggressive_burn; cost included_quota; policy reordered defaults over vibe/devstral-small; skipped vibe/devstral-small: quota_exhausted until tomorrow")
        );
    }

    #[test]
    fn paid_approval_description_does_not_invent_a_price() {
        let mut candidate = included_candidate();
        candidate.included_in_quota = false;
        candidate.requires_approval = true;
        candidate.quota_pool = None;
        candidate.priority = 0;

        assert_eq!(
            describe_candidate(&candidate, &PacingConfig::default()),
            "codex/gpt-5.4-mini (paid approval required)"
        );
    }
}
