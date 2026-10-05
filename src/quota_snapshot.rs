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

mod checks;
use checks::{build_freshness, build_quota_checks};
pub use checks::{QuotaCheck, QuotaCheckStatus};

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
    pub credential_id: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_usage: Option<crate::usage::account_usage::AccountUsageObservation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaCandidateStatus {
    pub modes: Vec<String>,
    pub backend: String,
    /// Billing/subscription service; the runner and model vendor may differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
    pub quota_checks: Vec<QuotaCheck>,
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
    pub quota_checked_at: Option<String>,
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

    let backend_groups = ledger::summary::build_grouped_summary(
        &entries,
        |entry| config::canonical_backend_name(&entry.effective_backend).to_string(),
        |observed| config::canonical_backend_name(observed.backend).to_string(),
        |backend, _model, _difficulty| config::canonical_backend_name(backend).to_string(),
    )
    .unwrap_or_default();
    let candidate_groups = ledger::summary::build_grouped_summary(
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
        |backend, model, _difficulty| {
            candidate_usage_key(config::canonical_backend_name(backend), model)
        },
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

    let freshness = build_freshness(ledger_observed_at, &candidates, &account_quota);
    let quota_checks = build_quota_checks(&account_quota);

    Ok(QuotaSnapshot {
        schema_version: 2,
        generated_at,
        freshness,
        quota_checks,
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
            add_candidate(
                routing,
                &mut aggregates,
                &mut index,
                "pm",
                candidate.clone(),
            );
        }
    }
    if let Some(list) = &routing.improve_candidates {
        for candidate in list {
            add_candidate(
                routing,
                &mut aggregates,
                &mut index,
                "improve",
                candidate.clone(),
            );
        }
    }
    if let Some(list) = &routing.review_candidates {
        for candidate in list {
            add_candidate(
                routing,
                &mut aggregates,
                &mut index,
                "review",
                candidate.clone(),
            );
        }
    }
    if let Some(candidate) = &routing.routine_reviewer {
        add_candidate(
            routing,
            &mut aggregates,
            &mut index,
            "routine_review",
            candidate.clone(),
        );
    }
    for candidate in &routing.escalatory_reviewers {
        add_candidate(
            routing,
            &mut aggregates,
            &mut index,
            "escalatory_review",
            candidate.clone(),
        );
    }

    if aggregates.is_empty() {
        if let Some(backend) = routing.default_backend.clone() {
            add_candidate(
                routing,
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
            let identity = routing.execution_identity_for_candidate(&aggregate.candidate);
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
                provider: quota_provider(
                    &identity.logical_backend,
                    identity.effective_model.as_deref(),
                ),
                backend: key.backend,
                backend_instance: Some(key.backend_instance),
                model: key.model,
                quota_pool: key.quota_pool,
                configured: aggregate
                    .candidate
                    .instance
                    .as_ref()
                    .and_then(|name| routing.backend_instances.get(name))
                    .is_some()
                    || (aggregate.candidate.instance.is_none()
                        && profile.is_backend_configured(&aggregate.candidate.backend)),
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

pub(super) fn quota_provider(backend: &str, model: Option<&str>) -> Option<String> {
    if model.is_some_and(|model| model.starts_with("nous-portal/")) {
        return Some("nous".to_string());
    }
    if model.is_some_and(|model| model.starts_with("gah-router/")) {
        return None; // The router's account inventory identifies the supplying subscription.
    }
    if backend == "mistral-dashboard" {
        return Some("mistral".into());
    }
    if matches!(backend, "agy" | "agy-main" | "agy-second") {
        return Some("antigravity".to_string());
    }
    crate::usage_attribution::provider_for_model(Some(backend), model)
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
    routing: &RoutingPolicy,
    aggregates: &mut Vec<(CandidateKey, CandidateAggregate)>,
    index: &mut HashMap<CandidateKey, usize>,
    mode: &str,
    candidate: CandidateConfig,
) {
    let identity = routing.execution_identity_for_candidate(&candidate);
    let key = CandidateKey {
        backend: identity.logical_backend,
        backend_instance: identity.backend_instance,
        model: identity.effective_model,
        quota_pool: identity.quota_pool,
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
    for account in quota_store::latest_windows_for_identity(account_quota, identity) {
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
            account_usage: account.account_usage.clone(),
            credential_id: account.credential_id.clone(),
        });
    }

    // A bound source may report a provider account distinct from its runner
    // instance/pool. Its exact credential ID owns the reading; broad ledger
    // aggregates have no source identity and cannot describe this account.
    // Unbound candidates retain backend, instance, pool and model scoping.
    out.retain(|observation| {
        if let Some(id) = identity.credential_id.as_deref() {
            return observation.credential_id.as_deref() == Some(id);
        }
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

/// #1339: group observations carry the shared store-record shape. For the
/// snapshot's display projection they stay deliberately unscoped (instance,
/// pool and credential are `None`) exactly as the former ledger summary
/// type did: a broad ledger aggregate has no verified source identity and
/// must not present itself as one account's balance.
fn convert_group_observation(obs: &crate::quota_store::QuotaObservationRecord) -> QuotaObservation {
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
        account_usage: None,
        credential_id: None,
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
mod tests;
