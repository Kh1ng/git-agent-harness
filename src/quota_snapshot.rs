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
    pub model: Option<String>,
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
    pub profile: ProfileIdentity,
    pub since: String,
    pub usage: UsageSummary,
    pub candidates: Vec<QuotaCandidateStatus>,
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
    let model_groups = ledger::summary::build_grouped_summary_with_account_quota(
        &entries,
        |entry| {
            entry
                .effective_model
                .clone()
                .unwrap_or_else(|| UNKNOWN_MODEL.to_string())
        },
        |observed| {
            observed
                .model
                .map(str::to_string)
                .unwrap_or_else(|| UNKNOWN_MODEL.to_string())
        },
        |_backend, model| {
            model
                .map(str::to_string)
                .unwrap_or_else(|| UNKNOWN_MODEL.to_string())
        },
        false,
        &account_quota,
    )
    .unwrap_or_default();

    let backend_map: HashMap<String, ledger::summary::GroupSummary> = backend_groups
        .into_iter()
        .map(|group| (group.group_key.clone(), group))
        .collect();
    let model_map: HashMap<String, ledger::summary::GroupSummary> = model_groups
        .into_iter()
        .map(|group| (group.group_key.clone(), group))
        .collect();

    let usage = summarize_groups(backend_map.values().cloned().collect());
    let candidates = build_candidates(
        &resolved_routing,
        profile,
        &backend_map,
        &model_map,
        &scope_lookup,
        &account_quota,
    );

    Ok(QuotaSnapshot {
        schema_version: 1,
        generated_at,
        profile: ProfileIdentity {
            profile: profile_name.to_string(),
            display_name: profile.display_name.clone(),
            repo_id: profile.repo_id.clone(),
            provider: profile.provider.clone(),
            local_path: profile.local_path.clone(),
            default_target_branch: profile.default_target_branch.clone(),
            merge_policy: resolved_routing.merge_policy.unwrap_or_default(),
        },
        since: since.to_string(),
        usage,
        candidates,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct CandidateKey {
    backend: String,
    model: Option<String>,
    quota_pool: Option<String>,
}

struct CandidateAggregate {
    modes: Vec<String>,
    candidate: CandidateConfig,
}

const UNKNOWN_MODEL: &str = "__unknown__";

fn build_candidates(
    routing: &RoutingPolicy,
    profile: &config::Profile,
    backend_map: &HashMap<String, ledger::summary::GroupSummary>,
    model_map: &HashMap<String, ledger::summary::GroupSummary>,
    scope_lookup: &HashMap<(String, Option<String>, Option<String>), availability::ScopeStatus>,
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
            let scope = scope_lookup
                .get(&(
                    key.backend.clone(),
                    key.model.clone(),
                    key.quota_pool.clone(),
                ))
                .cloned();
            let usage = aggregate_usage(
                backend_map.get(&key.backend),
                key.model.as_ref().and_then(|model| model_map.get(model)),
            );
            let mut quota_observations = aggregate_observations(
                backend_map.get(&key.backend),
                key.model.as_ref().and_then(|model| model_map.get(model)),
                account_quota,
                &key.backend,
                key.model.as_deref(),
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

fn add_candidate(
    aggregates: &mut Vec<(CandidateKey, CandidateAggregate)>,
    index: &mut HashMap<CandidateKey, usize>,
    mode: &str,
    candidate: CandidateConfig,
) {
    let key = CandidateKey {
        backend: config::canonical_backend_name(&candidate.backend).to_string(),
        model: candidate.model.clone(),
        quota_pool: candidate.quota_pool.clone(),
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
    backend: &str,
    model: Option<&str>,
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
    if let Some(account) = quota_store::latest_for(account_quota, backend, model) {
        out.push(QuotaObservation {
            backend: account.backend.clone(),
            model: account.model.clone(),
            quota_window: account.quota_window.clone(),
            quota_used_percent: account.quota_used_percent,
            quota_remaining_percent: account.quota_remaining_percent,
            quota_reset_at: account.quota_reset_at.clone(),
            observed_at: account.observed_at.clone(),
            usage_source: account.usage_source.clone(),
        });
    }

    let mut seen = BTreeSet::new();
    out.retain(|obs| {
        let key = (
            obs.backend.clone(),
            obs.model.clone(),
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
        model: obs.model.clone(),
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
