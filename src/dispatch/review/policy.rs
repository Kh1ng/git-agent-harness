use super::super::text::{extract_first_json_object, utf8_safe_prefix};
use super::context::ReviewDiffBundle;
use crate::config::{CandidateConfig, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::routing::RouteDecision;
use anyhow::Result;
use std::collections::HashSet;
use std::fmt;

/// Typed, terminal refusal used when a ticket has exhausted its configured
/// review budget. Keeping this distinct from backend failures lets the
/// controller close the run cleanly and makes the operator-visible event
/// stream explain that no reviewer was launched and no extra quota was spent.
#[derive(Debug)]
pub struct ReviewBudgetExhausted {
    pub(in crate::dispatch) reason: String,
}

impl ReviewBudgetExhausted {
    pub(in crate::dispatch) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ReviewBudgetExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for ReviewBudgetExhausted {}

pub fn review_budget_exhausted_error(err: &anyhow::Error) -> Option<&ReviewBudgetExhausted> {
    err.downcast_ref::<ReviewBudgetExhausted>()
}

/// A reviewer process completed, but its payload was not safe to use as
/// repair context. This is neither a backend crash nor a code finding: the
/// controller should retain the attempt and ask the next configured reviewer.
#[derive(Debug)]
pub struct ReviewOutputInvalid {
    reason: String,
}

impl ReviewOutputInvalid {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub(in crate::dispatch) fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ReviewOutputInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "review_output_invalid: {}", self.reason)
    }
}

impl std::error::Error for ReviewOutputInvalid {}

pub(in crate::dispatch) fn review_output_invalid_error(
    err: &anyhow::Error,
) -> Option<&ReviewOutputInvalid> {
    err.downcast_ref::<ReviewOutputInvalid>()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::dispatch) struct ReviewBudgetBlock {
    pub(in crate::dispatch) reason: String,
}

/// Return a deterministic ticket-scoped review budget block before a reviewer
/// is launched. A cycle is a prior review dispatch that consumed a real
/// reviewer call; it includes failed and timed-out reviews because those can
/// still consume quota, but excludes both a prior budget refusal and a
/// duplicate-review short-circuit (same source SHA/tier already reviewed),
/// plus an operator-requested shutdown, since none is a completed opinion.
/// Paid usage is counted only from an
/// explicit recorded `api_key_backed` classification, never inferred from a
/// provider name or silently from unknown data. The paid cap applies only
/// when routing has explicitly selected a candidate configured as paid;
/// quota-backed, local, and unknown-cost routes remain eligible until the
/// cycle cap is reached. The ordinary cycle cap bounds routine reviewers, but
/// each explicitly configured escalatory backend/model gets one real attempt
/// even when routine history already reached that cap. This exception remains
/// bounded by the finite escalation chain; an already-attempted escalatory
/// route cannot use it again, and paid-review limits are still checked below.
pub(in crate::dispatch) fn check_review_budget(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    work_id: Option<&str>,
    route: &RouteDecision,
) -> Result<Option<ReviewBudgetBlock>> {
    // Direct branch/MR reviews without a controller-provided ticket identity
    // cannot be attributed safely to a per-ticket budget. Fail open rather
    // than accidentally merging unrelated branches into one accounting bucket.
    let Some(work_id) = work_id.filter(|id| !id.trim().is_empty()) else {
        return Ok(None);
    };
    let routing = profile.effective_routing(&cfg.defaults);
    let entries = ledger::entries_for_work_id(cfg, work_id)?;
    // `clear-attempts` is an operator reset for this work item's bounded
    // automation budgets. Keep the append-only history, but do not let review
    // calls from before the latest matching tombstone permanently block the
    // ticket. Scope the reset to this profile/repository just like the review
    // records themselves so an identically-numbered issue elsewhere cannot
    // reset this budget.
    let active_entries = entries
        .iter()
        .rposition(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && entry.mode == "clear_attempts"
        })
        .map_or(entries.as_slice(), |index| &entries[index + 1..]);
    let reviews: Vec<_> = active_entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && entry.mode == "review"
                && entry.review_contract_version.unwrap_or(0)
                    >= crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                && !matches!(
                    entry.validation_result.as_deref(),
                    Some("review_budget_exhausted")
                        | Some("skipped_duplicate_review")
                        | Some("deferred_capacity")
                        | Some("cancelled_shutdown")
                )
        })
        .collect();

    let cycle_count = reviews.len() as u32;
    let cycle_cap = routing.max_review_cycles_per_ticket();
    let selected_tier = derive_reviewer_tier(cfg, profile, route);
    let selected_reviewer_class = reviewer_dedup_class(selected_tier, route);
    let selected_escalatory_reviewer_is_untried = selected_tier == ReviewerTier::Escalatory
        && !reviews.iter().any(|entry| {
            entry.reviewer_class.as_deref() == Some(selected_reviewer_class.as_str())
                || (entry.effective_backend == route.effective_backend
                    && entry.effective_model == route.effective_model)
        });
    if cycle_count >= cycle_cap && !selected_escalatory_reviewer_is_untried {
        return Ok(Some(ReviewBudgetBlock {
            reason: format!(
                "review budget exhausted for {work_id}: {cycle_count}/{cycle_cap} review cycles used"
            ),
        }));
    }

    let selected_paid = route
        .routing_diagnostics
        .as_ref()
        .and_then(|diagnostics| diagnostics.selected_cost_class.as_deref())
        == Some("paid");
    if selected_paid {
        let paid_count = reviews
            .iter()
            .filter(|entry| entry.usage.usage_classification.as_deref() == Some("api_key_backed"))
            .count() as u32;
        let paid_cap = routing.max_paid_reviews_per_ticket();
        if paid_count >= paid_cap {
            return Ok(Some(ReviewBudgetBlock {
                reason: format!(
                    "paid review budget exhausted for {work_id}: {paid_count}/{paid_cap} API-backed reviews used"
                ),
            }));
        }
    }

    Ok(None)
}

/// The routine reviewer (`review_backend`, e.g. Vibe/Mistral) is fast and
/// cheap but was never meant to be the last word on a genuinely hard or
/// repeatedly-failing review. The repeated-failure trigger follows the
/// configured post-review repair budget; adds an
/// immediate-escalate path for a reviewer that itself reported low
/// confidence, since forcing 2 low-confidence rubber stamps before getting
/// a second opinion defeats the point of tracking confidence at all.
///
/// Reads `validation_result`/`confidence_impact` off this branch's own
/// `mode == "review"` entries -- NOT `review_verdict`/`review_confidence`.
/// Those two fields are written by `backfill_review_verdict` (ledger/mod.rs,
/// TICKET-125) onto the *implementation* (fix/improve) entry instead, by
/// design (see `backfill_review_verdict_attributes_to_implementation_entry_not_reviewer`).
/// A review dispatch's own entry never carries a `review_verdict`, so
/// checking that field here would make this permanently a no-op; the
/// verdict/confidence a review entry actually records about itself live in
/// `validation_result`/`confidence_impact` (set directly in `review()`).
pub(in crate::dispatch) fn review_escalation_reason(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    branch: &str,
) -> Option<&'static str> {
    let repeated_failure_threshold = profile
        .effective_routing(&cfg.defaults)
        .max_fix_attempts_per_mr() as usize;

    let entries = ledger::read_entries(cfg).ok()?;
    let active_entries = active_branch_review_entries(&entries, profile, profile_name, branch);
    let recent: Vec<&LedgerEntry> = active_entries
        .iter()
        .rev()
        .filter(|e| {
            e.profile == profile_name
                && e.mode == "review"
                && e.branch.as_deref() == Some(branch)
                && e.review_contract_version.unwrap_or(0)
                    >= crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                // Legacy reviews written before reviewed-SHA persistence
                // cannot support a safe repair and must be re-runnable. Do
                // not let them trigger escalation before that migration
                // review has a chance to run.
                && e.review_source_sha.is_some()
                && matches!(
                    e.validation_result.as_deref(),
                    Some("APPROVE")
                        | Some("NEEDS_FIX")
                        | Some("REJECT")
                        | Some("HUMAN_REVIEW")
                        | Some("review_output_invalid")
                )
        })
        .take(repeated_failure_threshold)
        .collect();

    // A real HUMAN_REVIEW verdict and a deterministic evidence-gate hold both
    // use this persisted result. Neither is a reason to abandon automation
    // while a configured second-opinion reviewer remains.
    if recent
        .first()
        .is_some_and(|e| e.validation_result.as_deref() == Some("HUMAN_REVIEW"))
    {
        return Some("human_review");
    }

    if recent
        .first()
        .is_some_and(|e| e.validation_result.as_deref() == Some("review_output_invalid"))
    {
        return Some("review_output_invalid");
    }

    if recent
        .first()
        .is_some_and(|e| e.confidence_impact.as_deref() == Some("low"))
    {
        return Some("low_confidence");
    }

    if recent.len() == repeated_failure_threshold
        && recent.iter().all(|e| {
            matches!(
                e.validation_result.as_deref(),
                Some("NEEDS_FIX") | Some("REJECT")
            )
        })
    {
        return Some("repeated_needs_fix");
    }

    None
}

/// Invalid structured output is not an opinion, so it advances through the
/// complete ordered review pool (for example AGY account 1 -> AGY account 2
/// -> Claude) before the stronger/paid terminal escalation boundary. This is
/// intentionally broader than `next_escalatory_reviewer`, which handles a
/// valid but uncertain/adverse opinion.
pub(in crate::dispatch) fn next_review_candidate(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    branch: &str,
    current: Option<(&str, Option<&str>)>,
) -> Option<CandidateConfig> {
    let entries = ledger::read_entries(cfg).ok()?;
    let active_entries = active_branch_review_entries(&entries, profile, profile_name, branch);
    let mut attempted: HashSet<(String, Option<String>)> = active_entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
                && entry.review_source_sha.is_some()
                && entry.review_contract_version.unwrap_or(0)
                    >= crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                && entry.validation_result.as_deref() != Some("skipped_duplicate_review")
                && entry.validation_result.as_deref() != Some("cancelled_shutdown")
        })
        .map(|entry| {
            (
                entry.effective_backend.clone(),
                entry.effective_model.clone(),
            )
        })
        .collect();
    if let Some((backend, model)) = current {
        attempted.insert((backend.to_string(), model.map(str::to_string)));
    }

    profile
        .effective_routing(&cfg.defaults)
        .review_candidates
        .unwrap_or_default()
        .into_iter()
        .find(|candidate| {
            let effective_model = if candidate.backend == "codex" && candidate.model.is_none() {
                crate::runner::extract_model_from_args(&profile.codex_args)
            } else {
                candidate.model.clone()
            };
            !attempted.contains(&(candidate.backend.clone(), effective_model))
        })
}

/// Select the next unused reviewer from the explicitly ordered escalation
/// chain. The identity includes both backend instance and model: AGY account
/// 1, AGY account 2, and a paid gateway must remain independently observable
/// and independently eligible for a second opinion.
pub(in crate::dispatch) fn next_escalatory_reviewer(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    branch: &str,
    current: Option<(&str, Option<&str>)>,
) -> Option<CandidateConfig> {
    let entries = ledger::read_entries(cfg).ok()?;
    let active_entries = active_branch_review_entries(&entries, profile, profile_name, branch);
    let mut attempted: HashSet<(String, Option<String>)> = active_entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
                // A SHA-less legacy opinion is not reusable repair context.
                // Treating its backend as spent would skip directly to the
                // next (possibly paid) reviewer instead of migrating it.
                && entry.review_source_sha.is_some()
                && entry.review_contract_version.unwrap_or(0)
                    >= crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                && entry.validation_result.as_deref() != Some("skipped_duplicate_review")
                // An operator-requested shutdown is not a reviewer opinion
                // and must remain retryable after the daemon restarts.
                && entry.validation_result.as_deref() != Some("cancelled_shutdown")
        })
        .map(|entry| {
            (
                entry.effective_backend.clone(),
                entry.effective_model.clone(),
            )
        })
        .collect();
    if let Some((backend, model)) = current {
        attempted.insert((backend.to_string(), model.map(str::to_string)));
    }

    profile
        .effective_routing(&cfg.defaults)
        .effective_escalatory_reviewers()
        .into_iter()
        .find(|candidate| {
            // A candidate left without an explicit model is recorded in the
            // ledger under whatever effective model routing backfilled for it
            // (e.g. codex's config-file default, mirroring routing.rs's own
            // decide_route backfill) -- compare against that, not the raw
            // config value, or a once-tried backfilled candidate looks
            // perpetually untried and the chain never advances past it.
            let effective_model = if candidate.backend == "codex" && candidate.model.is_none() {
                crate::runner::extract_model_from_args(&profile.codex_args)
            } else {
                candidate.model.clone()
            };
            !attempted.contains(&(candidate.backend.clone(), effective_model))
        })
}

/// Return the append-only branch history after the latest operator reset for
/// a work item already attributed to this branch. Review escalation is branch
/// scoped, while `clear-attempts` tombstones are work-item scoped and carry no
/// branch. Deriving aliases from the branch's own review records bridges those
/// identities without allowing an identically numbered ticket in another
/// profile/repository to reset this chain.
fn active_branch_review_entries<'a>(
    entries: &'a [LedgerEntry],
    profile: &Profile,
    profile_name: &str,
    branch: &str,
) -> &'a [LedgerEntry] {
    let work_aliases: HashSet<String> = entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
        })
        .filter_map(|entry| entry.work_id.as_deref())
        .flat_map(ledger::work_id_aliases)
        .collect();

    let reset_index = (!work_aliases.is_empty()).then(|| {
        entries.iter().rposition(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && entry.mode == "clear_attempts"
                && entry.work_id.as_deref().is_some_and(|work_id| {
                    ledger::work_id_aliases(work_id)
                        .iter()
                        .any(|alias| work_aliases.contains(alias))
                })
        })
    });

    reset_index
        .flatten()
        .map_or(entries, |index| &entries[index + 1..])
}

/// Review deduplication normally works at the authority-tier level. An
/// ordered escalation chain deliberately contains several distinct second
/// opinions, so each escalatory backend/model pair gets one review of a
/// source commit rather than the first escalatory reviewer suppressing every
/// later one.
pub(in crate::dispatch) fn reviewer_dedup_class(
    tier: ReviewerTier,
    route: &RouteDecision,
) -> String {
    match tier {
        ReviewerTier::Escalatory => format!(
            "escalatory:{}/{}",
            route.effective_backend,
            route.effective_model.as_deref().unwrap_or("default")
        ),
        _ => tier.as_str().to_string(),
    }
}

/// Facts supplied by the control plane, not the reviewer. An approval must
/// cite these exact facts; free-form reviewer claims alone never make a change
/// safe to merge.
#[derive(Debug, Clone, Default)]
pub(in crate::dispatch) struct ReviewGateContext {
    changed_files: Vec<String>,
    ci_passed: bool,
    contract_files: Vec<String>,
    compatibility_mechanisms: Vec<&'static str>,
    acceptance_criteria: Vec<String>,
    source_provider: String,
    enforce_grounding: bool,
}

impl ReviewGateContext {
    pub(in crate::dispatch) fn from_diff_bundle(
        bundle: &ReviewDiffBundle,
        ci_status: Option<&str>,
    ) -> Self {
        let changed_files: Vec<String> = bundle
            .files
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        let diff_lower = bundle.diff.to_ascii_lowercase();
        let public_api_change = bundle.diff.lines().any(|line| {
            let line = line.trim_start_matches(['+', '-']);
            line.trim_start().starts_with("pub struct ")
                || line.trim_start().starts_with("pub enum ")
                || line.trim_start().starts_with("pub type ")
                || line.trim_start().starts_with("pub fn ")
        });
        let contract_files: Vec<String> = changed_files
            .iter()
            .filter(|path| {
                path.starts_with("packages/contracts/")
                    || path.starts_with("src/telemetry/")
                    || path == &"src/ledger/mod.rs"
                    || path.starts_with("migrations/")
                    || path.contains("/api/")
                    || path.starts_with("apps/server/src/")
                    || (public_api_change && path.starts_with("src/"))
            })
            .cloned()
            .collect();
        let mut compatibility_mechanisms = Vec::new();
        if diff_lower.contains("schema_version") {
            compatibility_mechanisms.push("schema-version");
        }
        if diff_lower.contains("serde(default)") {
            compatibility_mechanisms.push("backward-compatible-default");
        }
        if diff_lower.contains("migrat") {
            compatibility_mechanisms.push("migration");
        }

        Self {
            changed_files,
            ci_passed: ci_status.is_some_and(|status| {
                matches!(
                    status.trim().to_ascii_lowercase().as_str(),
                    "passed" | "success" | "green"
                )
            }),
            contract_files,
            compatibility_mechanisms,
            acceptance_criteria: Vec::new(),
            source_provider: String::new(),
            enforce_grounding: true,
        }
    }

    fn has_contract_surface_change(&self) -> bool {
        !self.contract_files.is_empty()
    }

    pub(in crate::dispatch) fn with_source_acceptance(
        mut self,
        acceptance_criteria: Vec<String>,
        provider: &str,
    ) -> Self {
        self.acceptance_criteria = acceptance_criteria;
        self.source_provider = provider.trim().to_ascii_lowercase();
        self
    }

    fn acceptance_gate_reason(&self, verdict: &crate::models::ReviewVerdict) -> Option<String> {
        if self.acceptance_criteria.is_empty() {
            return None;
        }

        if self
            .acceptance_criteria
            .iter()
            .any(|criterion| criterion_requires_external_state(criterion))
            && verdict
                .non_blocking_findings
                .iter()
                .chain(&verdict.risk_notes)
                .any(|finding| admits_unverified_state(finding))
        {
            return Some(
                "APPROVE admitted that required current/external acceptance state remained unverified"
                    .to_string(),
            );
        }

        for (index, criterion) in self.acceptance_criteria.iter().enumerate() {
            let number = index + 1;
            let prefix = format!("ac:{number}:");
            let mappings = verdict
                .evidence
                .iter()
                .filter_map(|item| item.trim().strip_prefix(&prefix))
                .collect::<Vec<_>>();
            if mappings.is_empty() {
                return Some(format!(
                    "APPROVE omitted evidence for source acceptance criterion {number}: {}",
                    utf8_safe_prefix(criterion, 160)
                ));
            }
            let external = criterion_requires_external_state(criterion);
            let valid = mappings
                .iter()
                .any(|mapping| self.acceptance_mapping_is_grounded(mapping, external));
            if !valid {
                return Some(format!(
                    "APPROVE evidence for source acceptance criterion {number} was not grounded{}",
                    if external {
                        " in direct provider evidence or a testable changed snapshot"
                    } else {
                        " in a changed file or concrete test result"
                    }
                ));
            }
        }
        None
    }

    fn acceptance_mapping_is_grounded(&self, mapping: &str, external: bool) -> bool {
        if let Some(rest) = mapping.strip_prefix("provider:") {
            let Some((provider, reference)) = rest.split_once(':') else {
                return false;
            };
            return external
                && !reference.trim().is_empty()
                && provider.trim().eq_ignore_ascii_case(&self.source_provider);
        }
        if let Some(rest) = mapping.strip_prefix("snapshot:") {
            let Some((path, verification)) = rest.split_once(':') else {
                return false;
            };
            return external
                && !verification.trim().is_empty()
                && self
                    .changed_files
                    .iter()
                    .any(|candidate| candidate == path.trim());
        }
        if external {
            return false;
        }
        if let Some(path) = mapping.strip_prefix("file:") {
            return self.mapping_references_changed_file(path);
        }
        mapping
            .strip_prefix("test:")
            .is_some_and(|detail| !detail.trim().is_empty())
    }

    fn mapping_references_changed_file(&self, mapping: &str) -> bool {
        let mapping = mapping.trim();
        self.changed_files.iter().any(|candidate| {
            let Some(suffix) = mapping.strip_prefix(candidate) else {
                return false;
            };
            suffix.is_empty()
                || suffix.starts_with(" — ")
                || suffix.starts_with(" – ")
                || suffix.starts_with(" - ")
                || suffix.starts_with(" (")
                || suffix.starts_with(" [")
        })
    }

    fn evidence_is_grounded(&self, evidence: &[String]) -> bool {
        evidence.iter().any(|item| {
            let Some(path) = item.trim().strip_prefix("file:") else {
                return false;
            };
            self.changed_files
                .iter()
                .any(|candidate| candidate == path.trim())
        })
    }

    fn falsely_claims_passed_ci(&self, evidence: &[String]) -> bool {
        !self.ci_passed
            && evidence
                .iter()
                .any(|item| item.trim().eq_ignore_ascii_case("ci:passed"))
    }

    fn compatibility_is_grounded(&self, evidence: &[String]) -> bool {
        evidence.iter().any(|item| {
            let Some(path) = item.trim().strip_prefix("file:") else {
                return false;
            };
            self.contract_files
                .iter()
                .any(|candidate| candidate == path.trim())
        }) && evidence.iter().any(|item| {
            let Some(mechanism) = item.trim().strip_prefix("mechanism:") else {
                return false;
            };
            self.compatibility_mechanisms
                .iter()
                .any(|candidate| candidate == &mechanism.trim())
        })
    }
}

#[allow(clippy::too_many_arguments)]
/// TICKET-108: reviewer authority (who is reviewing) kept as a dimension
/// separate from review outcome (verdict/confidence, what they said).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::dispatch) enum ReviewerTier {
    Strong,
    Standard,
    Weak,
    /// Issue #123: an escalatory reviewer (a more-capable model from the
    /// ESCALATORY_REVIEW list) the pipeline escalated to and continued with.
    /// Auto-merge eligible like `Strong`, but recorded distinctly so the
    /// cascade origin is observable.
    Escalatory,
}

impl ReviewerTier {
    pub(in crate::dispatch) fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Standard => "standard",
            Self::Weak => "weak",
            Self::Escalatory => "escalatory",
        }
    }
}

/// Derived from which configured routing field actually selected this
/// backend/model, not from anything the reviewer says about itself -- a
/// weak reviewer cannot self-promote by returning confident-sounding text
/// (TICKET-108's core requirement).
pub(in crate::dispatch) fn derive_reviewer_tier(
    cfg: &GahConfig,
    profile: &Profile,
    route: &RouteDecision,
) -> ReviewerTier {
    let effective_model = route.effective_model.as_deref();
    let selected = |backend_cfg: Option<&str>, model_cfg: Option<&str>| -> bool {
        backend_cfg.is_some_and(|b| b == route.effective_backend)
            && (model_cfg.is_none() || model_cfg == effective_model)
    };
    let routine = profile
        .routing
        .effective_routine_reviewer()
        .or_else(|| cfg.defaults.routing.effective_routine_reviewer());
    let escalatory = profile
        .routing
        .escalatory_reviewers
        .iter()
        .cloned()
        .chain(cfg.defaults.routing.escalatory_reviewers.clone())
        .collect::<Vec<_>>();

    // Issue #233: tier classification must only honor explicitly declared
    // escalatory reviewers. The legacy weak-review keys still feed routing
    // backfill via `effective_escalatory_reviewers()`, but they do not imply
    // the auto-merge-eligible escalatory tier.
    for esc in &escalatory {
        if selected(Some(esc.backend.as_str()), esc.model.as_deref()) {
            // Check if this escalatory reviewer is actually a legacy weak review configuration
            // Legacy weak review configs should be treated as Weak tier, not Escalatory
            let is_legacy_weak_config = profile.routing.escalatory_reviewers.is_empty()
                && profile.routing.weak_review_backend.as_deref() == Some(esc.backend.as_str())
                && profile.routing.weak_review_model.as_deref() == esc.model.as_deref();

            if is_legacy_weak_config {
                return ReviewerTier::Weak;
            }
            return ReviewerTier::Escalatory;
        }
    }
    // Routine reviewer is the STRONG first-line authority.
    if let Some(routine) = &routine {
        if selected(Some(routine.backend.as_str()), routine.model.as_deref()) {
            return ReviewerTier::Strong;
        }
    }
    let strong_backend = profile.routing.strong_review_backend.as_deref().or(cfg
        .defaults
        .routing
        .strong_review_backend
        .as_deref());
    let strong_model = profile.routing.strong_review_model.as_deref().or(cfg
        .defaults
        .routing
        .strong_review_model
        .as_deref());
    let weak_backend = profile.routing.weak_review_backend.as_deref().or(cfg
        .defaults
        .routing
        .weak_review_backend
        .as_deref());
    let weak_model = profile.routing.weak_review_model.as_deref().or(cfg
        .defaults
        .routing
        .weak_review_model
        .as_deref());

    if selected(weak_backend, weak_model) {
        return ReviewerTier::Weak;
    }
    if selected(strong_backend, strong_model) {
        return ReviewerTier::Strong;
    }
    // review_candidates is the operator's actual declared pool of reviewers
    // they consider trustworthy (agy/agy-second/claude serving the same
    // Sonnet-class model are routinely interchangeable fallbacks for each
    // other, not different capability tiers). Requiring strong_review_backend/
    // model to be manually kept in sync with every review_candidates entry
    // is exactly the kind of drift that already produced two real bugs
    // tonight (gah's own strong_review_backend pointed at codex-mini; here,
    // falling back from agy to agy-second/claude silently downgraded a
    // Sonnet reviewer to "standard" tier). Any candidate not already
    // classified weak above is strong.
    let candidates = profile.routing.review_candidates.as_ref().or(cfg
        .defaults
        .routing
        .review_candidates
        .as_ref());
    if let Some(candidates) = candidates {
        let in_candidates = candidates.iter().any(|c| {
            c.backend == route.effective_backend
                && (c.model.is_none() || c.model.as_deref() == effective_model)
        });
        if in_candidates {
            return ReviewerTier::Strong;
        }
    }
    ReviewerTier::Standard
}

#[cfg(test)]
fn parse_review_verdict(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
    tier: ReviewerTier,
) -> Result<crate::models::ReviewVerdict> {
    parse_review_verdict_with_context(
        review_text,
        route,
        parsed_usage,
        tier,
        &ReviewGateContext::default(),
    )
}

pub(in crate::dispatch) fn parse_review_verdict_with_context(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
    tier: ReviewerTier,
    gate_context: &ReviewGateContext,
) -> Result<crate::models::ReviewVerdict> {
    let json = extract_first_json_object(review_text)
        .ok_or_else(|| anyhow::anyhow!("reviewer did not return verdict JSON"))?;
    let mut verdict = serde_json::from_str::<crate::models::ReviewVerdict>(&json)?;
    normalize_actionable_findings(&mut verdict, gate_context)?;
    enforce_review_evidence_gate(
        &mut verdict,
        review_text,
        &route.effective_backend,
        gate_context,
    );
    // Reviewer identity (tier) and review outcome (verdict text/confidence)
    // are separate dimensions -- the verdict text itself is never rewritten
    // based on who reviewed it (see review_labels for how tier affects
    // labeling instead).
    if tier == ReviewerTier::Weak && verdict.confidence == "high" {
        // Weak approval is deliberately not auto-merge authority. A weak
        // reviewer finding a defect is actionable input for the normal
        // post-review repair budget and must not skip straight to a human.
        verdict.confidence = "medium".into();
    }
    if tier == ReviewerTier::Weak && verdict.verdict == "APPROVE" {
        verdict.human_required = true;
    }
    if verdict.verdict == "HUMAN_REVIEW"
        || (verdict.verdict == "APPROVE" && verdict.confidence == "low")
    {
        verdict.human_required = true;
    }
    verdict.reviewer_tier = Some(tier.as_str().to_string());
    verdict.reviewer_backend = Some(route.effective_backend.clone());
    verdict.reviewer_model = route.effective_model.clone();
    verdict.requested_backend = Some(route.requested_backend.clone());
    verdict.effective_backend = Some(route.effective_backend.clone());
    verdict.requested_model = route.requested_model.clone();
    verdict.effective_model = route.effective_model.clone();
    verdict.fallback_used = Some(route.fallback_used);
    verdict.usage_source = parsed_usage.usage_source.clone();
    verdict.input_tokens = parsed_usage.input_tokens;
    verdict.output_tokens = parsed_usage.output_tokens;
    verdict.total_tokens = parsed_usage.total_tokens;
    verdict.estimated_cost_usd = parsed_usage.estimated_cost_usd;
    verdict.actual_cost_usd = parsed_usage.actual_cost_usd;
    Ok(verdict)
}

fn normalize_actionable_findings(
    verdict: &mut crate::models::ReviewVerdict,
    gate_context: &ReviewGateContext,
) -> Result<()> {
    let repair_verdict = matches!(verdict.verdict.as_str(), "NEEDS_FIX" | "REJECT");
    if repair_verdict && verdict.actionable_findings.is_empty() {
        // The ungrounded context exists only for parser-focused unit tests and
        // historical compatibility helpers. Production review always builds
        // a diff-backed context and therefore takes the strict branch below.
        if !gate_context.enforce_grounding {
            return Ok(());
        }
        return Err(
            ReviewOutputInvalid::new("NEEDS_FIX/REJECT omitted actionable_findings").into(),
        );
    }
    if verdict.actionable_findings.is_empty() {
        return Ok(());
    }

    let mut rendered = Vec::with_capacity(verdict.actionable_findings.len());
    for (index, finding) in verdict.actionable_findings.iter().enumerate() {
        let number = index + 1;
        if !finding.status.trim().eq_ignore_ascii_case("confirmed") {
            return Err(ReviewOutputInvalid::new(format!(
                "actionable finding {number} status must be confirmed"
            ))
            .into());
        }
        let file = finding.file.trim();
        if file.is_empty()
            || (gate_context.enforce_grounding
                && !gate_context.changed_files.iter().any(|path| path == file))
        {
            return Err(ReviewOutputInvalid::new(format!(
                "actionable finding {number} did not name an exact changed file"
            ))
            .into());
        }
        let summary = finding.summary.trim();
        if summary.is_empty() {
            return Err(ReviewOutputInvalid::new(format!(
                "actionable finding {number} omitted its summary"
            ))
            .into());
        }
        if finding_text_disclaims_actionability(summary)
            || finding
                .evidence
                .iter()
                .any(|evidence| finding_text_disclaims_actionability(evidence))
        {
            return Err(ReviewOutputInvalid::new(format!(
                "actionable finding {number} explicitly withdrew, contradicted, or left the finding unverified"
            ))
            .into());
        }
        let grounded = finding.evidence.iter().any(|item| {
            let Some(rest) = item.trim().strip_prefix("diff:") else {
                return false;
            };
            let Some((evidence_file, detail)) = rest.split_once(':') else {
                return false;
            };
            evidence_file.trim() == file && !detail.trim().is_empty()
        });
        if !grounded {
            return Err(ReviewOutputInvalid::new(format!(
                "actionable finding {number} omitted direct diff:{file}:<observation> evidence"
            ))
            .into());
        }

        let location = finding
            .line
            .as_deref()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map_or_else(|| file.to_string(), |line| format!("{file}:{line}"));
        rendered.push(format!("{location}: {summary}"));
    }
    verdict.blocking_findings = rendered;
    Ok(())
}

fn finding_text_disclaims_actionability(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    [
        "withdrawn",
        "not blocking",
        "actually fine",
        "no issue here",
        "cannot be confirmed",
        "can't be confirmed",
        "unverified risk",
        "unverified concern",
        "partially unverified",
        "may or may not",
        "not strictly broken",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

/// A reviewer is advisory; merge safety is deterministic. In particular, an
/// LLM must not be able to write an apparent APPROVE while its own structured
/// findings describe a blocking or unversioned contract change (the exact
/// failure observed in PR #284). The normalized verdict remains visible in
/// the review artifact, ledger, and status payload.
fn enforce_review_evidence_gate(
    verdict: &mut crate::models::ReviewVerdict,
    review_text: &str,
    reviewer_backend: &str,
    gate_context: &ReviewGateContext,
) {
    if verdict.verdict != "APPROVE" {
        return;
    }

    if let Some(reason) = gate_context.acceptance_gate_reason(verdict) {
        verdict.verdict = "NEEDS_FIX".to_string();
        verdict.human_required = false;
        if !verdict.blocking_findings.contains(&reason) {
            verdict.blocking_findings.push(reason.clone());
        }
        verdict.safety_gate_reason = Some(reason);
        return;
    }

    let reason = if !verdict.blocking_findings.is_empty() {
        Some("APPROVE contradicted non-empty blocking_findings".to_string())
    } else if review_text_has_substantive_prose(review_text, reviewer_backend) {
        Some(
            "APPROVE included substantive prose; every finding must be represented in the review JSON"
                .to_string(),
        )
    } else if verdict.evidence.is_empty() {
        Some("APPROVE omitted required concrete review evidence".to_string())
    } else if gate_context.enforce_grounding
        && gate_context.falsely_claims_passed_ci(&verdict.evidence)
    {
        Some("APPROVE claimed passed CI while the control plane did not report it".to_string())
    } else if gate_context.enforce_grounding
        && !gate_context.evidence_is_grounded(&verdict.evidence)
    {
        Some(
            "APPROVE evidence was not grounded in an exact changed file from the control plane"
                .to_string(),
        )
    } else if gate_context.has_contract_surface_change()
        && (gate_context.compatibility_mechanisms.is_empty()
            || !gate_context.compatibility_is_grounded(&verdict.compatibility_evidence))
    {
        Some(
            "APPROVE changed a contract surface without a control-plane-verifiable compatibility mechanism and evidence"
                .to_string(),
        )
    } else {
        None
    };

    let Some(reason) = reason else {
        return;
    };

    verdict.verdict = "HUMAN_REVIEW".to_string();
    verdict.human_required = true;
    verdict.safety_gate_reason = Some(reason);
}

fn criterion_requires_external_state(criterion: &str) -> bool {
    let lower = criterion.to_ascii_lowercase();
    [
        "current",
        "live",
        "latest",
        "stale",
        "open issue",
        "closed issue",
        "queue",
        "backlog",
        "github",
        "gitlab",
        "provider state",
        "remaining quota",
        "availability",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn admits_unverified_state(finding: &str) -> bool {
    let lower = finding.to_ascii_lowercase();
    [
        "not verified",
        "not re-verified",
        "not reverified",
        "unverified",
        "not checked",
        "could be stale",
        "may be stale",
        "might be stale",
        "unable to verify",
        "could not verify",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn review_text_has_substantive_prose(review_text: &str, reviewer_backend: &str) -> bool {
    let Some(json) = extract_first_json_object(review_text) else {
        return true;
    };
    let Some(start) = review_text.find(&json) else {
        return true;
    };
    let mut residue = String::with_capacity(review_text.len().saturating_sub(json.len()));
    residue.push_str(&review_text[..start]);
    residue.push_str(&review_text[start + json.len()..]);
    let agy_transport_trace = matches!(reviewer_backend, "agy" | "agy-second");
    residue.lines().map(str::trim).any(|line| {
        // `agy --print` writes its execution-plan trace to stdout before
        // the final answer. Those uniform "I will ..." lines are runner
        // transport metadata, not reviewer prose. Preserve fail-closed
        // behavior for every other line, including AGY's final prose.
        let inert = line.is_empty()
            || (agy_transport_trace && line.starts_with("I will "))
            || matches!(
                line.to_ascii_lowercase().trim_end_matches(':').trim(),
                "review notes" | "## review notes" | "### review notes" | "```json" | "```"
            );
        !inert
    })
}

#[cfg(test)]
mod tests;
