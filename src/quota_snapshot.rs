use crate::availability;
use crate::config::{self, CandidateConfig, GahConfig, RoutingPolicy};
use crate::ledger::{self, LedgerEntry};
use crate::quota_store;
use crate::status::ProfileIdentity;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageSummary {
    pub entries: usize,
    pub attempts: usize,
    pub validation_pass: usize,
    pub success_rate: Option<f64>,
    pub total_tokens: Option<u64>,
    pub requests_count: Option<u64>,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaObservation {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_reset_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaCandidateStatus {
    pub modes: Vec<String>,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    pub configured: bool,
    pub eligible_now: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub usage: UsageSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota_observations: Vec<QuotaObservation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaSnapshot {
    pub schema_version: u32,
    pub generated_at: String,
    pub freshness: QuotaFreshness,
    pub profile: ProfileIdentity,
    pub since: String,
    pub usage: UsageSummary,
    pub candidates: Vec<QuotaCandidateStatus>,
}

/// Latest source evidence behind a snapshot. `generated_at` only proves the
/// command ran; these timestamps tell operators how old the underlying facts
/// are without pretending that an empty recent window is fresh telemetry.
#[derive(Debug, Clone, Serialize, Default)]
pub struct QuotaFreshness {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger_observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability_observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_observed_at: Option<String>,
}

pub fn run(cfg: &GahConfig, profile_name: &str, since: &str, json: bool) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let snapshot = build_snapshot(cfg, profile_name, since, now)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        println!("Quota snapshot for Profile: {}", profile_name);
        println!("Window: last {}", since);
        println!(
            "Usage: entries={} validation_pass={} tokens={} requests={} success={}",
            snapshot.usage.entries,
            snapshot.usage.validation_pass,
            snapshot
                .usage
                .total_tokens
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            snapshot
                .usage
                .requests_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            snapshot
                .usage
                .success_rate
                .map(|n| format!("{:.1}%", n * 100.0))
                .unwrap_or_else(|| "unknown".to_string())
        );
        for candidate in &snapshot.candidates {
            println!(
                "  - {}{}{}: {}{}",
                candidate.backend,
                candidate
                    .model
                    .as_deref()
                    .map(|m| format!("/{m}"))
                    .unwrap_or_default(),
                candidate
                    .quota_pool
                    .as_deref()
                    .map(|pool| format!(" [{pool}]"))
                    .unwrap_or_default(),
                if candidate.eligible_now {
                    "eligible".to_string()
                } else {
                    candidate
                        .reason
                        .as_deref()
                        .unwrap_or("unavailable")
                        .to_string()
                },
                candidate
                    .quota_observations
                    .first()
                    .and_then(|o| o.quota_window.as_deref())
                    .map(|w| format!(" (window: {w})"))
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}

pub fn build_snapshot(
    cfg: &GahConfig,
    profile_name: &str,
    since: &str,
    now: OffsetDateTime,
) -> Result<QuotaSnapshot> {
    let profile = config::get_profile(cfg, profile_name)?;
    let generated_at = now.format(&Rfc3339).unwrap_or_default();
    let resolved_routing = profile.effective_routing(&cfg.defaults);
    let cutoff = ledger::summary::parse_since(since)?;

    let mut entries = ledger::read_entries(cfg)?;
    let ledger_observed_at = latest_timestamp(
        entries
            .iter()
            .filter(|entry| entry.profile == profile_name)
            .map(|entry| entry.timestamp.clone()),
    );
    entries.retain(|entry| entry.profile == profile_name && entry.timestamp >= cutoff);

    let account_quota = quota_store::load_account_observations();
    let state_path = availability::resolve_state_path();
    let scope_statuses = availability::list_scopes(&state_path, now)?;
    let scope_lookup = scope_statuses
        .into_iter()
        .map(|scope| {
            (
                (
                    scope.backend.clone(),
                    scope.backend_instance.clone(),
                    scope.model.clone(),
                    scope.quota_pool.clone(),
                ),
                scope,
            )
        })
        .collect::<HashMap<_, _>>();

    let backend_groups = ledger::summary::build_grouped_summary_with_account_quota(
        &entries,
        |entry| config::canonical_backend_name(&entry.effective_backend).to_string(),
        |observed| config::canonical_backend_name(observed.backend).to_string(),
        |backend, _model| config::canonical_backend_name(backend).to_string(),
        true,
        &account_quota,
    )
    .unwrap_or_default();
    let candidate_groups = ledger::summary::build_grouped_summary_with_account_quota(
        &entries,
        |entry| {
            candidate_usage_key(
                config::canonical_backend_name(&entry.effective_backend),
                entry.effective_model.as_deref(),
            )
        },
        |observed| {
            candidate_usage_key(
                config::canonical_backend_name(observed.backend),
                observed.model,
            )
        },
        |backend, model| candidate_usage_key(config::canonical_backend_name(backend), model),
        false,
        &account_quota,
    )
    .unwrap_or_default();

    let backend_map: HashMap<String, ledger::summary::GroupSummary> = backend_groups
        .into_iter()
        .map(|group| (group.group_key.clone(), group))
        .collect();
    let candidate_map: HashMap<String, ledger::summary::GroupSummary> = candidate_groups
        .into_iter()
        .map(|group| (group.group_key.clone(), group))
        .collect();

    let usage = summarize_groups(backend_map.values().cloned().collect());
    let candidates = build_candidates(
        &resolved_routing,
        profile,
        &backend_map,
        &candidate_map,
        &scope_lookup,
        &account_quota,
    );

    let freshness = QuotaFreshness {
        ledger_observed_at,
        availability_observed_at: latest_timestamp(
            candidates
                .iter()
                .filter_map(|candidate| candidate.observed_at.clone()),
        ),
        quota_observed_at: latest_timestamp(
            candidates
                .iter()
                .flat_map(|candidate| candidate.quota_observations.iter())
                .filter_map(|observation| observation.observed_at.clone()),
        ),
    };

    Ok(QuotaSnapshot {
        schema_version: 2,
        generated_at,
        freshness,
        profile: ProfileIdentity {
            profile: profile_name.to_string(),
            display_name: profile.display_name.clone(),
            repo_id: profile.repo_id.clone(),
            provider: profile.provider.clone(),
            local_path: profile.local_path.clone(),
            default_target_branch: profile.default_target_branch.clone(),
            merge_policy: resolved_routing.merge_policy.unwrap_or_default(),
            max_fix_attempts_per_mr: resolved_routing.max_fix_attempts_per_mr(),
            max_implementation_failures_per_ticket: resolved_routing
                .max_implementation_failures_per_ticket(),
            max_open_managed_mrs: profile.max_open_managed_mrs(),
            issue_intake_policy: crate::models::IssueIntakePolicy {
                mode: profile.publishing.issue_intake_mode.as_str().to_string(),
                canonical_autonomous_label: profile.publishing.canonical_autonomous_label.clone(),
                trusted_human_authors: profile
                    .publishing
                    .trusted_issue_human_authors
                    .clone()
                    .or_else(|| profile.publishing.github_issue_author_allowlist.clone())
                    .unwrap_or_else(|| {
                        profile
                            .repo
                            .split_once('/')
                            .map(|(owner, _)| vec![owner.to_string()])
                            .unwrap_or_default()
                    }),
                trusted_bot_authors: profile
                    .publishing
                    .trusted_issue_bot_authors
                    .clone()
                    .unwrap_or_default(),
                github_issue_author_allowlist: profile
                    .publishing
                    .github_issue_author_allowlist
                    .clone()
                    .unwrap_or_default(),
            },
        },
        since: since.to_string(),
        usage,
        candidates,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct CandidateKey {
    backend: String,
    backend_instance: String,
    model: Option<String>,
    quota_pool: Option<String>,
}

struct CandidateAggregate {
    modes: Vec<String>,
    candidate: CandidateConfig,
}

type AvailabilityScopeLookup =
    HashMap<(String, Option<String>, Option<String>, Option<String>), availability::ScopeStatus>;

const UNKNOWN_MODEL: &str = "__unknown__";

fn candidate_usage_key(backend: &str, model: Option<&str>) -> String {
    format!("{backend}\u{1f}{}", model.unwrap_or(UNKNOWN_MODEL))
}

fn latest_timestamp(values: impl Iterator<Item = String>) -> Option<String> {
    values
        .filter_map(|value| {
            OffsetDateTime::parse(&value, &Rfc3339)
                .ok()
                .map(|parsed| (parsed, value))
        })
        .max_by_key(|(parsed, _)| *parsed)
        .map(|(_, value)| value)
}

fn build_candidates(
    routing: &RoutingPolicy,
    profile: &config::Profile,
    backend_map: &HashMap<String, ledger::summary::GroupSummary>,
    candidate_map: &HashMap<String, ledger::summary::GroupSummary>,
    scope_lookup: &AvailabilityScopeLookup,
    account_quota: &[quota_store::QuotaObservationRecord],
) -> Vec<QuotaCandidateStatus> {
    let mut aggregates: Vec<(CandidateKey, CandidateAggregate)> = Vec::new();
    let mut index: HashMap<CandidateKey, usize> = HashMap::new();

    if let Some(list) = &routing.pm_candidates {
        for candidate in list {
            add_candidate(&mut aggregates, &mut index, "pm", candidate.clone());
        }
    }
    if let Some(list) = &routing.improve_candidates {
        for candidate in list {
            add_candidate(&mut aggregates, &mut index, "improve", candidate.clone());
        }
    }
    if let Some(list) = &routing.review_candidates {
        for candidate in list {
            add_candidate(&mut aggregates, &mut index, "review", candidate.clone());
        }
    }
    if let Some(candidate) = &routing.routine_reviewer {
        add_candidate(
            &mut aggregates,
            &mut index,
            "routine_review",
            candidate.clone(),
        );
    }
    for candidate in &routing.escalatory_reviewers {
        add_candidate(
            &mut aggregates,
            &mut index,
            "escalatory_review",
            candidate.clone(),
        );
    }

    if aggregates.is_empty() {
        if let Some(backend) = routing.default_backend.clone() {
            add_candidate(
                &mut aggregates,
                &mut index,
                "default",
                CandidateConfig {
                    backend,
                    model: routing.default_model.clone(),
                    quota_pool: None,
                    ..CandidateConfig::default()
                },
            );
        }
    }

    aggregates
        .into_iter()
        .map(|(key, aggregate)| {
            let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
                &key.backend,
                key.model.as_deref(),
                key.quota_pool.as_deref(),
            );
            identity.backend_instance = key.backend_instance.clone();
            let scope = find_scope_status(scope_lookup, &key);
            let candidate_group = key.model.as_deref().and_then(|model| {
                candidate_map.get(&candidate_usage_key(&key.backend, Some(model)))
            });
            let usage = aggregate_usage(backend_map.get(&key.backend), candidate_group);
            let mut quota_observations = aggregate_observations(
                backend_map.get(&key.backend),
                candidate_group,
                account_quota,
                &identity,
            );
            quota_observations.sort_by(|left, right| {
                left.quota_window
                    .cmp(&right.quota_window)
                    .then_with(|| right.observed_at.cmp(&left.observed_at))
                    .then_with(|| left.usage_source.cmp(&right.usage_source))
            });

            QuotaCandidateStatus {
                modes: aggregate.modes,
                backend: key.backend,
                backend_instance: Some(key.backend_instance),
                model: key.model,
                quota_pool: key.quota_pool,
                configured: profile.is_backend_configured(&aggregate.candidate.backend),
                eligible_now: scope.as_ref().map(|s| s.eligible).unwrap_or(true),
                reason: scope
                    .as_ref()
                    .and_then(|s| s.reason.map(|r| r.as_str().to_string())),
                unavailable_until: scope.as_ref().and_then(|s| s.unavailable_until.clone()),
                source: scope
                    .as_ref()
                    .and_then(|s| s.source.map(|s| s.as_str().to_string())),
                last_error_summary: scope.as_ref().and_then(|s| s.last_error_summary.clone()),
                observed_at: scope.as_ref().and_then(|s| s.observed_at.clone()),
                usage,
                quota_observations,
            }
        })
        .collect()
}

fn find_scope_status(
    scope_lookup: &AvailabilityScopeLookup,
    key: &CandidateKey,
) -> Option<availability::ScopeStatus> {
    if let Some(pool) = key.quota_pool.as_deref() {
        if let Some(status) = scope_lookup
            .values()
            .find(|status| status.quota_pool.as_deref() == Some(pool))
        {
            return Some(status.clone());
        }
    }
    for (instance, model, pool) in [
        (
            Some(key.backend_instance.clone()),
            key.model.clone(),
            key.quota_pool.clone(),
        ),
        (None, key.model.clone(), key.quota_pool.clone()),
        (Some(key.backend_instance.clone()), key.model.clone(), None),
        (None, key.model.clone(), None),
        (Some(key.backend_instance.clone()), None, None),
        (None, None, None),
    ] {
        if let Some(status) = scope_lookup.get(&(key.backend.clone(), instance, model, pool)) {
            return Some(status.clone());
        }
    }
    None
}

fn add_candidate(
    aggregates: &mut Vec<(CandidateKey, CandidateAggregate)>,
    index: &mut HashMap<CandidateKey, usize>,
    mode: &str,
    candidate: CandidateConfig,
) {
    let effective_quota_pool = availability::resolve_candidate_quota_pool(
        &candidate.backend,
        candidate.model.as_deref(),
        candidate.quota_pool.as_deref(),
    );
    let key = CandidateKey {
        backend: config::canonical_backend_name(&candidate.backend).to_string(),
        backend_instance: crate::execution_identity::legacy_backend_instance(
            config::canonical_backend_name(&candidate.backend),
            effective_quota_pool.as_deref(),
        ),
        model: candidate.model.clone(),
        quota_pool: effective_quota_pool,
    };
    if let Some(idx) = index.get(&key).copied() {
        let modes = &mut aggregates[idx].1.modes;
        if !modes.iter().any(|m| m == mode) {
            modes.push(mode.to_string());
        }
        return;
    }
    let aggregate = CandidateAggregate {
        modes: vec![mode.to_string()],
        candidate,
    };
    index.insert(key.clone(), aggregates.len());
    aggregates.push((key, aggregate));
}

fn aggregate_usage(
    backend_group: Option<&ledger::summary::GroupSummary>,
    model_group: Option<&ledger::summary::GroupSummary>,
) -> UsageSummary {
    let group = model_group.or(backend_group);
    group
        .map(|g| UsageSummary {
            entries: g.entries,
            attempts: g.attempts,
            validation_pass: g.validation_pass,
            success_rate: g.success_rate,
            total_tokens: g.total_tokens,
            requests_count: g.requests_count,
            actual_cost_usd: g.actual_cost_usd,
            estimated_cost_usd: g.estimated_cost_usd,
        })
        .unwrap_or_default()
}

fn aggregate_observations(
    backend_group: Option<&ledger::summary::GroupSummary>,
    model_group: Option<&ledger::summary::GroupSummary>,
    account_quota: &[quota_store::QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Vec<QuotaObservation> {
    let mut out = Vec::new();
    if let Some(group) = backend_group {
        out.extend(
            group
                .quota_observations
                .iter()
                .map(convert_group_observation),
        );
    }
    if let Some(group) = model_group {
        out.extend(
            group
                .quota_observations
                .iter()
                .map(convert_group_observation),
        );
    }
    if let Some(account) = quota_store::latest_for_identity(account_quota, identity) {
        out.push(QuotaObservation {
            backend: account.backend.clone(),
            backend_instance: account.backend_instance.clone(),
            model: account.model.clone(),
            quota_pool: account.quota_pool.clone(),
            quota_window: account.quota_window.clone(),
            quota_used_percent: account.quota_used_percent,
            quota_remaining_percent: account.quota_remaining_percent,
            quota_reset_at: account.quota_reset_at.clone(),
            observed_at: account.observed_at.clone(),
            usage_source: account.usage_source.clone(),
        });
    }

    // Backend and model summaries are intentionally broad aggregates. Filter
    // after combining them so a same-named model on another backend, another
    // model on this backend, or a sibling account cannot leak into this card.
    out.retain(|observation| {
        config::canonical_backend_name(&observation.backend) == identity.logical_backend
            && observation
                .backend_instance
                .as_deref()
                .is_none_or(|instance| instance == identity.backend_instance)
            && observation
                .quota_pool
                .as_deref()
                .is_none_or(|pool| Some(pool) == identity.quota_pool.as_deref())
            && match identity.effective_model.as_deref() {
                Some(model) => observation
                    .model
                    .as_deref()
                    .is_none_or(|value| value == model),
                None => observation.model.is_none(),
            }
    });

    let mut seen = BTreeSet::new();
    out.retain(|obs| {
        let key = (
            obs.backend.clone(),
            obs.backend_instance.clone(),
            obs.model.clone(),
            obs.quota_pool.clone(),
            obs.quota_window.clone(),
            obs.quota_used_percent.map(f64::to_bits),
            obs.quota_remaining_percent.map(f64::to_bits),
            obs.quota_reset_at.clone(),
            obs.observed_at.clone(),
            obs.usage_source.clone(),
        );
        seen.insert(key)
    });
    out
}

fn convert_group_observation(obs: &ledger::summary::GroupQuotaObservation) -> QuotaObservation {
    QuotaObservation {
        backend: obs.backend.clone(),
        backend_instance: None,
        model: obs.model.clone(),
        quota_pool: None,
        quota_window: obs.quota_window.clone(),
        quota_used_percent: obs.quota_used_percent,
        quota_remaining_percent: obs.quota_remaining_percent,
        quota_reset_at: obs.quota_reset_at.clone(),
        observed_at: obs.observed_at.clone(),
        usage_source: obs.usage_source.clone(),
    }
}

fn summarize_groups(groups: Vec<ledger::summary::GroupSummary>) -> UsageSummary {
    let mut summary = UsageSummary::default();
    for group in groups {
        summary.entries += group.entries;
        summary.attempts += group.attempts;
        summary.validation_pass += group.validation_pass;
        if let Some(tokens) = group.total_tokens {
            summary.total_tokens = Some(summary.total_tokens.unwrap_or(0) + tokens);
        }
        if let Some(count) = group.requests_count {
            summary.requests_count = Some(summary.requests_count.unwrap_or(0) + count);
        }
        if let Some(cost) = group.actual_cost_usd {
            summary.actual_cost_usd = Some(summary.actual_cost_usd.unwrap_or(0.0) + cost);
        }
        if let Some(cost) = group.estimated_cost_usd {
            summary.estimated_cost_usd = Some(summary.estimated_cost_usd.unwrap_or(0.0) + cost);
        }
    }
    if summary.entries > 0 {
        summary.success_rate = Some(summary.validation_pass as f64 / summary.entries as f64);
    }
    summary
}

fn entry_matches_candidate(entry: &LedgerEntry, backend: &str, model: Option<&str>) -> bool {
    if config::canonical_backend_name(&entry.effective_backend) != backend {
        return false;
    }
    match model {
        Some(model) => entry.effective_model.as_deref() == Some(model),
        None => true,
    }
}

#[allow(dead_code)]
fn filtered_entries<'a>(
    entries: &'a [LedgerEntry],
    backend: &str,
    model: Option<&str>,
) -> Vec<&'a LedgerEntry> {
    entries
        .iter()
        .filter(|entry| entry_matches_candidate(entry, backend, model))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::{BlockScope, Reason, ScopeStatus, Source};
    use crate::config::tests::test_profile_for_notifications;
    use crate::ledger::summary::GroupQuotaObservation;

    /// A `GroupSummary` with every field zeroed/empty, so individual tests
    /// only spell out the fields they actually care about via struct-update
    /// syntax (`GroupSummary { entries: 3, ..empty_group() }`). No `Default`
    /// impl exists on the production type (see `ledger/mod.rs`), so this mirrors
    /// that module's own fixture convention.
    fn empty_group() -> ledger::summary::GroupSummary {
        ledger::summary::GroupSummary {
            group_key: "g".to_string(),
            entries: 0,
            attempts: 0,
            attempts_started: None,
            attempts_completed: None,
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            validation_pass: 0,
            success_rate: None,
            review_verdict_distribution: Default::default(),
            total_cost_usd: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
            average_cost_usd: None,
            average_duration_seconds: None,
            cost_per_approve_strong: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
            tokens_per_success: None,
            requests_per_success: None,
            quota_observations: vec![],
        }
    }

    fn group_obs(
        backend: &str,
        model: Option<&str>,
        window: &str,
        remaining_percent: Option<f64>,
        observed_at: &str,
    ) -> GroupQuotaObservation {
        GroupQuotaObservation {
            backend: backend.to_string(),
            model: model.map(str::to_string),
            quota_window: Some(window.to_string()),
            quota_used_percent: None,
            quota_remaining_percent: remaining_percent,
            quota_reset_at: None,
            observed_at: Some(observed_at.to_string()),
            usage_source: None,
        }
    }

    fn account_record(
        backend: &str,
        model: Option<&str>,
        window: &str,
        remaining_percent: Option<f64>,
        observed_at: &str,
    ) -> quota_store::QuotaObservationRecord {
        quota_store::QuotaObservationRecord {
            backend: backend.to_string(),
            backend_instance: None,
            model: model.map(str::to_string),
            quota_pool: None,
            quota_window: Some(window.to_string()),
            quota_used_percent: None,
            quota_remaining_percent: remaining_percent,
            quota_reset_at: None,
            observed_at: Some(observed_at.to_string()),
            usage_source: None,
        }
    }

    fn availability_status(
        backend: &str,
        backend_instance: Option<&str>,
        model: Option<&str>,
        quota_pool: Option<&str>,
        eligible: bool,
        scope: Option<BlockScope>,
    ) -> ScopeStatus {
        ScopeStatus {
            backend: backend.into(),
            backend_instance: backend_instance.map(str::to_string),
            model: model.map(str::to_string),
            quota_pool: quota_pool.map(str::to_string),
            eligible,
            reason: (!eligible).then_some(Reason::QuotaExhausted),
            unavailable_until: None,
            scope,
            source: Some(Source::Imported),
            last_error_summary: None,
            observed_at: Some("2026-07-20T00:00:00Z".into()),
        }
    }

    #[test]
    fn candidate_status_inherits_backend_wide_and_cross_instance_pool_state() {
        let candidate = CandidateKey {
            backend: "opencode".into(),
            backend_instance: "opencode-account-a".into(),
            model: Some("shared-model".into()),
            quota_pool: None,
        };
        let mut lookup = AvailabilityScopeLookup::new();
        lookup.insert(
            ("opencode".into(), None, None, None),
            availability_status(
                "opencode",
                None,
                None,
                None,
                false,
                Some(BlockScope::BackendWide),
            ),
        );
        assert!(!find_scope_status(&lookup, &candidate).unwrap().eligible);

        let pooled = CandidateKey {
            quota_pool: Some("shared-pool".into()),
            ..candidate
        };
        lookup.insert(
            (
                "opencode".into(),
                Some("opencode-account-b".into()),
                Some("other-model".into()),
                Some("shared-pool".into()),
            ),
            availability_status(
                "opencode",
                Some("opencode-account-b"),
                Some("other-model"),
                Some("shared-pool"),
                true,
                None,
            ),
        );
        assert!(find_scope_status(&lookup, &pooled).unwrap().eligible);
    }

    // -- summarize_groups --------------------------------------------------

    #[test]
    fn summarize_groups_sums_across_groups_and_computes_success_rate() {
        let a = ledger::summary::GroupSummary {
            entries: 4,
            attempts: 5,
            validation_pass: 3,
            total_tokens: Some(100),
            requests_count: Some(10),
            actual_cost_usd: Some(1.5),
            estimated_cost_usd: Some(0.5),
            ..empty_group()
        };
        let b = ledger::summary::GroupSummary {
            entries: 6,
            attempts: 6,
            validation_pass: 3,
            total_tokens: Some(200),
            requests_count: Some(20),
            actual_cost_usd: Some(2.0),
            estimated_cost_usd: None,
            ..empty_group()
        };
        let summary = summarize_groups(vec![a, b]);
        assert_eq!(summary.entries, 10);
        assert_eq!(summary.attempts, 11);
        assert_eq!(summary.validation_pass, 6);
        assert_eq!(summary.total_tokens, Some(300));
        assert_eq!(summary.requests_count, Some(30));
        assert!((summary.actual_cost_usd.unwrap() - 3.5).abs() < f64::EPSILON);
        // Only `a` has an estimated cost; `b`'s None must not zero it out.
        assert!((summary.estimated_cost_usd.unwrap() - 0.5).abs() < f64::EPSILON);
        assert!((summary.success_rate.unwrap() - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn summarize_groups_empty_input_has_no_success_rate_or_totals() {
        let summary = summarize_groups(vec![]);
        assert_eq!(summary.entries, 0);
        assert_eq!(summary.success_rate, None);
        assert_eq!(summary.total_tokens, None);
        assert_eq!(summary.requests_count, None);
    }

    #[test]
    fn summarize_groups_all_none_token_fields_stay_none_not_zero() {
        // Regression guard: a group that never reported tokens must leave the
        // aggregate at `None` ("unknown"), not silently become `Some(0)`.
        let a = ledger::summary::GroupSummary {
            entries: 2,
            attempts: 2,
            validation_pass: 1,
            ..empty_group()
        };
        let summary = summarize_groups(vec![a]);
        assert_eq!(summary.total_tokens, None);
        assert_eq!(summary.requests_count, None);
        assert_eq!(summary.actual_cost_usd, None);
    }

    #[test]
    fn latest_timestamp_compares_instants_instead_of_rfc3339_text() {
        let latest = latest_timestamp(
            [
                "2026-07-20T23:00:00-05:00".to_string(),
                "2026-07-21T02:00:00Z".to_string(),
            ]
            .into_iter(),
        );
        assert_eq!(latest.as_deref(), Some("2026-07-20T23:00:00-05:00"));
    }

    // -- aggregate_usage -----------------------------------------------------

    #[test]
    fn aggregate_usage_prefers_model_group_over_backend_group() {
        let backend = ledger::summary::GroupSummary {
            entries: 10,
            ..empty_group()
        };
        let model = ledger::summary::GroupSummary {
            entries: 3,
            ..empty_group()
        };
        let usage = aggregate_usage(Some(&backend), Some(&model));
        assert_eq!(usage.entries, 3);
    }

    #[test]
    fn aggregate_usage_falls_back_to_backend_group_when_no_model_group() {
        let backend = ledger::summary::GroupSummary {
            entries: 10,
            ..empty_group()
        };
        let usage = aggregate_usage(Some(&backend), None);
        assert_eq!(usage.entries, 10);
    }

    #[test]
    fn aggregate_usage_defaults_when_neither_group_present() {
        let usage = aggregate_usage(None, None);
        assert_eq!(usage.entries, 0);
        assert_eq!(usage.success_rate, None);
    }

    // -- aggregate_observations ----------------------------------------------

    #[test]
    fn aggregate_observations_combines_backend_and_model_group_observations() {
        let backend = ledger::summary::GroupSummary {
            quota_observations: vec![group_obs(
                "codex",
                None,
                "weekly",
                Some(50.0),
                "2026-07-01T00:00:00Z",
            )],
            ..empty_group()
        };
        let model = ledger::summary::GroupSummary {
            quota_observations: vec![group_obs(
                "codex",
                Some("gpt-5"),
                "5h",
                Some(80.0),
                "2026-07-02T00:00:00Z",
            )],
            ..empty_group()
        };
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "codex",
            Some("gpt-5"),
            None::<String>,
        );
        let obs = aggregate_observations(Some(&backend), Some(&model), &[], &identity);
        assert_eq!(obs.len(), 2);
        assert!(obs
            .iter()
            .any(|o| o.quota_window.as_deref() == Some("weekly")));
        assert!(obs.iter().any(|o| o.quota_window.as_deref() == Some("5h")));
    }

    #[test]
    fn aggregate_observations_appends_matching_account_level_observation() {
        let account = vec![account_record(
            "codex",
            None,
            "weekly",
            Some(42.0),
            "2026-07-03T00:00:00Z",
        )];
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "codex",
            None::<String>,
            None::<String>,
        );
        let obs = aggregate_observations(None, None, &account, &identity);
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].quota_remaining_percent, Some(42.0));
    }

    #[test]
    fn aggregate_observations_does_not_leak_account_observation_across_model_scope() {
        // Candidate scoping: an account-level record for "gpt-4" must not
        // surface on a "gpt-5" candidate's observations just because the
        // backend matches.
        let account = vec![account_record(
            "codex",
            Some("gpt-4"),
            "weekly",
            Some(42.0),
            "2026-07-03T00:00:00Z",
        )];
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "codex",
            Some("gpt-5"),
            None::<String>,
        );
        let obs = aggregate_observations(None, None, &account, &identity);
        assert!(obs.is_empty());
    }

    #[test]
    fn aggregate_observations_filters_broad_group_data_to_candidate_identity() {
        let backend = ledger::summary::GroupSummary {
            quota_observations: vec![
                group_obs(
                    "agy",
                    Some("Gemini"),
                    "daily",
                    Some(70.0),
                    "2026-07-03T00:00:00Z",
                ),
                group_obs(
                    "agy",
                    Some("Claude"),
                    "daily",
                    Some(20.0),
                    "2026-07-03T00:00:00Z",
                ),
                group_obs(
                    "agy-second",
                    Some("Gemini"),
                    "daily",
                    Some(5.0),
                    "2026-07-03T00:00:00Z",
                ),
            ],
            ..empty_group()
        };

        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "agy",
            Some("Gemini"),
            None::<String>,
        );
        let obs = aggregate_observations(Some(&backend), None, &[], &identity);

        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].backend, "agy");
        assert_eq!(obs[0].model.as_deref(), Some("Gemini"));
        assert_eq!(obs[0].quota_remaining_percent, Some(70.0));
    }

    #[test]
    fn aggregate_observations_dedups_identical_entries_from_backend_and_model_groups() {
        let dup = group_obs("codex", None, "weekly", Some(50.0), "2026-07-01T00:00:00Z");
        let backend = ledger::summary::GroupSummary {
            quota_observations: vec![dup.clone()],
            ..empty_group()
        };
        let model = ledger::summary::GroupSummary {
            quota_observations: vec![dup],
            ..empty_group()
        };
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "codex",
            None::<String>,
            None::<String>,
        );
        let obs = aggregate_observations(Some(&backend), Some(&model), &[], &identity);
        assert_eq!(obs.len(), 1, "identical observations must collapse to one");
    }

    // -- add_candidate --------------------------------------------------------

    #[test]
    fn add_candidate_merges_modes_for_the_same_key_without_duplicating() {
        let mut aggregates = Vec::new();
        let mut index = HashMap::new();
        let candidate = CandidateConfig {
            backend: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            ..Default::default()
        };
        add_candidate(&mut aggregates, &mut index, "pm", candidate.clone());
        add_candidate(&mut aggregates, &mut index, "improve", candidate.clone());
        add_candidate(&mut aggregates, &mut index, "pm", candidate);

        assert_eq!(aggregates.len(), 1);
        assert_eq!(aggregates[0].1.modes, vec!["pm", "improve"]);
    }

    #[test]
    fn add_candidate_treats_different_quota_pools_as_distinct_candidates() {
        // Candidate scoping: "agy" and "agy-second" are different instances
        // and must never collapse into a single row (see QuotaPage.tsx's own
        // `scopeIdentity` doc comment for the same invariant on the UI side).
        let mut aggregates = Vec::new();
        let mut index = HashMap::new();
        let a = CandidateConfig {
            backend: "agy".to_string(),
            quota_pool: Some("agy".to_string()),
            ..Default::default()
        };
        let b = CandidateConfig {
            backend: "agy".to_string(),
            quota_pool: Some("agy-second".to_string()),
            ..Default::default()
        };
        add_candidate(&mut aggregates, &mut index, "review", a);
        add_candidate(&mut aggregates, &mut index, "review", b);

        assert_eq!(aggregates.len(), 2);
    }

    // -- build_candidates -----------------------------------------------------

    #[test]
    fn build_candidates_falls_back_to_default_backend_when_none_configured() {
        let routing = RoutingPolicy {
            default_backend: Some("vibe".to_string()),
            default_model: Some("mistral-medium".to_string()),
            ..RoutingPolicy::default()
        };
        let profile = test_profile_for_notifications();
        let backend_map = HashMap::new();
        let model_map = HashMap::new();
        let scope_lookup = HashMap::new();
        let account_quota = vec![];

        let candidates = build_candidates(
            &routing,
            &profile,
            &backend_map,
            &model_map,
            &scope_lookup,
            &account_quota,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].backend, "vibe");
        assert_eq!(candidates[0].modes, vec!["default"]);
        assert!(
            candidates[0].eligible_now,
            "no availability record for this scope must default to eligible, not blocked"
        );
    }

    #[test]
    fn build_candidates_keeps_distinct_quota_pools_separately_scoped() {
        // Two candidates sharing a backend but different quota_pool must not
        // share eligibility -- one being blocked must not leak onto the other.
        let routing = RoutingPolicy {
            review_candidates: Some(vec![
                CandidateConfig {
                    backend: "agy".to_string(),
                    quota_pool: Some("agy".to_string()),
                    ..Default::default()
                },
                CandidateConfig {
                    backend: "agy".to_string(),
                    quota_pool: Some("agy-second".to_string()),
                    ..Default::default()
                },
            ]),
            ..RoutingPolicy::default()
        };
        let profile = test_profile_for_notifications();
        let backend_map = HashMap::new();
        let model_map = HashMap::new();
        let mut scope_lookup = HashMap::new();
        scope_lookup.insert(
            ("agy".to_string(), None, None, Some("agy".to_string())),
            ScopeStatus {
                backend: "agy".to_string(),
                backend_instance: None,
                model: None,
                quota_pool: Some("agy".to_string()),
                eligible: false,
                reason: Some(Reason::QuotaExhausted),
                unavailable_until: Some("2026-07-12T00:00:00Z".to_string()),
                scope: Some(BlockScope::QuotaPool),
                source: Some(Source::BackendError),
                last_error_summary: None,
                observed_at: Some("2026-07-11T00:00:00Z".to_string()),
            },
        );
        let account_quota = vec![];

        let candidates = build_candidates(
            &routing,
            &profile,
            &backend_map,
            &model_map,
            &scope_lookup,
            &account_quota,
        );

        assert_eq!(candidates.len(), 2);
        let agy = candidates
            .iter()
            .find(|c| c.quota_pool.as_deref() == Some("agy"))
            .expect("agy pool present");
        assert!(!agy.eligible_now);
        assert_eq!(agy.reason.as_deref(), Some("quota_exhausted"));

        let agy_second = candidates
            .iter()
            .find(|c| c.quota_pool.as_deref() == Some("agy-second"))
            .expect("agy-second pool present");
        assert!(
            agy_second.eligible_now,
            "a block on the 'agy' pool must not leak onto the sibling 'agy-second' pool"
        );
    }

    #[test]
    fn build_candidates_sorts_quota_observations_by_window_then_recency() {
        let routing = RoutingPolicy {
            default_backend: Some("codex".to_string()),
            ..RoutingPolicy::default()
        };
        let profile = test_profile_for_notifications();
        let mut backend_map = HashMap::new();
        backend_map.insert(
            "codex".to_string(),
            ledger::summary::GroupSummary {
                quota_observations: vec![
                    group_obs("codex", None, "weekly", Some(10.0), "2026-07-01T00:00:00Z"),
                    group_obs("codex", None, "5h", Some(90.0), "2026-07-02T00:00:00Z"),
                ],
                ..empty_group()
            },
        );
        let model_map = HashMap::new();
        let scope_lookup = HashMap::new();
        let account_quota = vec![];

        let candidates = build_candidates(
            &routing,
            &profile,
            &backend_map,
            &model_map,
            &scope_lookup,
            &account_quota,
        );

        assert_eq!(candidates.len(), 1);
        let windows: Vec<String> = candidates[0]
            .quota_observations
            .iter()
            .map(|o| o.quota_window.clone().unwrap())
            .collect();
        assert_eq!(windows, vec!["5h".to_string(), "weekly".to_string()]);
    }

    #[test]
    fn build_candidates_scopes_same_named_model_usage_by_backend() {
        let routing = RoutingPolicy {
            review_candidates: Some(vec![
                CandidateConfig {
                    backend: "codex".to_string(),
                    model: Some("shared-name".to_string()),
                    ..Default::default()
                },
                CandidateConfig {
                    backend: "agy".to_string(),
                    model: Some("shared-name".to_string()),
                    ..Default::default()
                },
            ]),
            ..RoutingPolicy::default()
        };
        let profile = test_profile_for_notifications();
        let mut candidate_map = HashMap::new();
        candidate_map.insert(
            candidate_usage_key("codex", Some("shared-name")),
            ledger::summary::GroupSummary {
                entries: 3,
                ..empty_group()
            },
        );
        candidate_map.insert(
            candidate_usage_key("agy", Some("shared-name")),
            ledger::summary::GroupSummary {
                entries: 9,
                ..empty_group()
            },
        );

        let candidates = build_candidates(
            &routing,
            &profile,
            &HashMap::new(),
            &candidate_map,
            &HashMap::new(),
            &[],
        );

        assert_eq!(candidates[0].usage.entries, 3);
        assert_eq!(candidates[1].usage.entries, 9);
    }

    #[test]
    fn build_candidates_creates_four_distinct_agy_scopes() {
        let routing = RoutingPolicy {
            pm_candidates: Some(vec![
                CandidateConfig {
                    backend: "agy".to_string(),
                    model: Some("Gemini 3.5 Flash (Medium)".to_string()),
                    quota_pool: None,
                    ..Default::default()
                },
                CandidateConfig {
                    backend: "agy".to_string(),
                    model: Some("Claude Sonnet 4.6 (Thinking)".to_string()),
                    quota_pool: None,
                    ..Default::default()
                },
                CandidateConfig {
                    backend: "agy-second".to_string(),
                    model: Some("Gemini 3.5 Flash".to_string()),
                    quota_pool: Some("agy-second".to_string()),
                    ..Default::default()
                },
                CandidateConfig {
                    backend: "agy-second".to_string(),
                    model: Some("Claude Sonnet 4.6".to_string()),
                    quota_pool: None,
                    ..Default::default()
                },
            ]),
            ..RoutingPolicy::default()
        };
        let profile = test_profile_for_notifications();
        let candidates = build_candidates(
            &routing,
            &profile,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[],
        );

        assert_eq!(candidates.len(), 4);
        let pools: Vec<Option<String>> = candidates.iter().map(|c| c.quota_pool.clone()).collect();
        assert_eq!(
            pools,
            vec![
                Some("agy:google-native".to_string()),
                Some("agy:external".to_string()),
                Some("agy-second:google-native".to_string()),
                Some("agy-second:external".to_string()),
            ]
        );
    }
}
