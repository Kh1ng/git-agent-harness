use crate::config;
use crate::ledger::entry::{LedgerEntry, LedgerUsage};
use crate::ledger::jsonl::read_entries;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use time::{Duration, OffsetDateTime};

/// TICKET-071: stable machine-readable aggregate ledger data. Shared by
/// both the human-readable and `--json` output paths so they can never
/// drift apart -- no speculative economics logic, just the counts the
/// human view already computed.
/// TICKET-125: Grouped summary data for a specific backend or model
#[derive(Debug, Serialize, Clone)]
pub struct GroupSummary {
    pub group_key: String,
    pub entries: usize,
    pub attempts: usize,
    /// Issue #240: attempt counters are `Option<u32>` on `LedgerEntry`, so
    /// an unknown (pre-tracking) entry is excluded from the sum rather than
    /// counted as `0`. `None` means no entry in this group had a known
    /// value. Legacy (pre-tracking) entries are counted in `*-unknown`.
    pub attempts_started: Option<u32>,
    pub attempts_completed: Option<u32>,
    pub attempts_started_unknown: usize,
    pub attempts_completed_unknown: usize,
    pub validation_pass: usize,
    /// Validation success divided by executions in this backend/model
    /// group. `None` means no executions were observed.
    pub success_rate: Option<f64>,
    pub review_verdict_distribution: BTreeMap<String, usize>,
    pub total_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub average_cost_usd: Option<f64>,
    pub average_duration_seconds: Option<f64>,
    pub cost_per_approve_strong: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub requests_count: Option<u64>,
    pub tokens_per_success: Option<f64>,
    pub requests_per_success: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota_observations: Vec<GroupQuotaObservation>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GroupQuotaObservation {
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

#[derive(Debug, Serialize, Clone)]
pub struct SummaryData {
    pub ledger_path: String,
    pub entries: usize,
    pub success: usize,
    pub failed: usize,
    pub by_mode: BTreeMap<String, usize>,
    pub by_requested_backend: BTreeMap<String, usize>,
    pub by_backend: BTreeMap<String, usize>,
    pub by_model: BTreeMap<String, usize>,
    pub by_failure_class: BTreeMap<String, usize>,
    pub fallback_count: usize,
    pub validation_pass: usize,
    pub push_success: usize,
    pub mr_count: usize,
    pub human_required_count: usize,
    /// Issue #240: attempt counters are `Option<u32>` on `LedgerEntry`, so
    /// an unknown (pre-tracking) entry is excluded from the sum rather than
    /// counted as `0`. `None` means no entry had a known value. Legacy
    /// (pre-tracking) entries are counted in `*-unknown`.
    pub attempts_started: Option<u32>,
    pub attempts_completed: Option<u32>,
    pub attempts_started_unknown: usize,
    pub attempts_completed_unknown: usize,
    pub average_duration_seconds: Option<f64>,
    pub usage_input_tokens: Option<u64>,
    pub usage_output_tokens: Option<u64>,
    pub usage_cache_read_tokens: Option<u64>,
    pub usage_cache_write_tokens: Option<u64>,
    pub usage_total_tokens: Option<u64>,
    pub usage_requests_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,
    pub last_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grouped_by_backend: Option<Vec<GroupSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grouped_by_model: Option<Vec<GroupSummary>>,
}

pub fn run_with_json(
    since: &str,
    profile: Option<&str>,
    config_path: Option<&str>,
    json: bool,
    group_by: super::GroupBy,
) -> Result<()> {
    let cfg = config::load(config_path)?;
    let data = build_summary(&cfg, since, profile, group_by)?;

    if json {
        println!("{}", serde_json::to_string(&data)?);
        return Ok(());
    }

    println!("Ledger: {}", data.ledger_path);
    println!("Entries: {}", data.entries);
    if data.entries == 0 {
        return Ok(());
    }
    println!("Success: {}", data.success);
    println!("Failed:  {}", data.failed);
    println!("By mode:");
    print_counts(&data.by_mode);
    println!("Requested backend:");
    print_counts(&data.by_requested_backend);
    println!("Backend:");
    print_counts(&data.by_backend);
    println!("Model:");
    print_counts(&data.by_model);
    println!("Failure class:");
    print_counts(&data.by_failure_class);
    println!("Fallback: {}", data.fallback_count);
    println!("Validation pass: {}", data.validation_pass);
    println!("Push success: {}", data.push_success);
    println!("MR count: {}", data.mr_count);
    println!("Human required: {}", data.human_required_count);
    if let Some(avg) = data.average_duration_seconds {
        println!("Average duration: {:.2}s", avg);
    }
    if let Some(input) = data.usage_input_tokens {
        println!("Input tokens: {}", input);
    }
    if let Some(output) = data.usage_output_tokens {
        println!("Output tokens: {}", output);
    }
    if let Some(cache_read) = data.usage_cache_read_tokens {
        println!("Cache read tokens: {}", cache_read);
    }
    if let Some(cache_write) = data.usage_cache_write_tokens {
        println!("Cache write tokens: {}", cache_write);
    }
    if let Some(total) = data.usage_total_tokens {
        println!("Total tokens: {}", total);
    }
    if let Some(count) = data.usage_requests_count {
        println!("Requests count: {}", count);
    }
    if let Some(estimated) = data.estimated_cost_usd {
        println!("Estimated cost: ${:.4}", estimated);
    }
    if let Some(actual) = data.actual_cost_usd {
        println!("Actual cost: ${:.4}", actual);
    }
    if let Some(avg) = data
        .attempts_started
        .zip(data.attempts_completed)
        .map(|(started, completed)| started as f64 / completed as f64)
    {
        println!("Attempts per completion: {:.2}", avg);
    }
    if let Some(avg_duration) = data.average_duration_seconds {
        println!("Average duration: {:.2}s", avg_duration);
    }
    if let Some(avg_tokens) = data
        .usage_input_tokens
        .zip(data.usage_output_tokens)
        .map(|(input, output)| (input + output) as f64 / data.success as f64)
    {
        println!("Tokens per success: {:.2}", avg_tokens);
    }
    if let Some(avg_requests) = data
        .usage_requests_count
        .map(|count| count as f64 / data.success as f64)
    {
        println!("Requests per success: {:.2}", avg_requests);
    }
    if let Some(ref grouped) = data.grouped_by_backend {
        println!("\nGrouped by backend:");
        for group in grouped {
            println!("  {}:", group.group_key);
            println!("    Entries: {}", group.entries);
            println!("    Attempts: {}", group.attempts);
            if let Some(started) = group.attempts_started {
                println!("    Attempts started: {}", started);
            }
            if let Some(completed) = group.attempts_completed {
                println!("    Attempts completed: {}", completed);
            }
            println!(
                "    Attempts started unknown: {}",
                group.attempts_started_unknown
            );
            println!(
                "    Attempts completed unknown: {}",
                group.attempts_completed_unknown
            );
            println!("    Validation pass: {}", group.validation_pass);
            if let Some(rate) = group.success_rate {
                println!("    Success rate: {:.2}%", rate * 100.0);
            }
            if let Some(cost) = group.total_cost_usd {
                println!("    Total cost: ${:.4}", cost);
            }
            if let Some(cost) = group.actual_cost_usd {
                println!("    Actual cost: ${:.4}", cost);
            }
            if let Some(cost) = group.estimated_cost_usd {
                println!("    Estimated cost: ${:.4}", cost);
            }
            if let Some(avg) = group.average_cost_usd {
                println!("    Average cost: ${:.4}", avg);
            }
            if let Some(avg) = group.average_duration_seconds {
                println!("    Average duration: {:.2}s", avg);
            }
            if let Some(input) = group.input_tokens {
                println!("    Input tokens: {}", input);
            }
            if let Some(output) = group.output_tokens {
                println!("    Output tokens: {}", output);
            }
            if let Some(cache_read) = group.cache_read_tokens {
                println!("    Cache read tokens: {}", cache_read);
            }
            if let Some(cache_write) = group.cache_write_tokens {
                println!("    Cache write tokens: {}", cache_write);
            }
            if let Some(total) = group.total_tokens {
                println!("    Total tokens: {}", total);
            }
            if let Some(count) = group.requests_count {
                println!("    Requests count: {}", count);
            }
            if let Some(tokens) = group.tokens_per_success {
                println!("    Tokens per success: {:.2}", tokens);
            }
            if let Some(requests) = group.requests_per_success {
                println!("    Requests per success: {:.2}", requests);
            }
        }
    }
    if let Some(ref grouped) = data.grouped_by_model {
        println!("\nGrouped by model:");
        for group in grouped {
            println!("  {}:", group.group_key);
            println!("    Entries: {}", group.entries);
            println!("    Attempts: {}", group.attempts);
            if let Some(started) = group.attempts_started {
                println!("    Attempts started: {}", started);
            }
            if let Some(completed) = group.attempts_completed {
                println!("    Attempts completed: {}", completed);
            }
            println!(
                "    Attempts started unknown: {}",
                group.attempts_started_unknown
            );
            println!(
                "    Attempts completed unknown: {}",
                group.attempts_completed_unknown
            );
            println!("    Validation pass: {}", group.validation_pass);
            if let Some(rate) = group.success_rate {
                println!("    Success rate: {:.2}%", rate * 100.0);
            }
            if let Some(cost) = group.total_cost_usd {
                println!("    Total cost: ${:.4}", cost);
            }
            if let Some(cost) = group.actual_cost_usd {
                println!("    Actual cost: ${:.4}", cost);
            }
            if let Some(cost) = group.estimated_cost_usd {
                println!("    Estimated cost: ${:.4}", cost);
            }
            if let Some(avg) = group.average_cost_usd {
                println!("    Average cost: ${:.4}", avg);
            }
            if let Some(avg) = group.average_duration_seconds {
                println!("    Average duration: {:.2}s", avg);
            }
            if let Some(input) = group.input_tokens {
                println!("    Input tokens: {}", input);
            }
            if let Some(output) = group.output_tokens {
                println!("    Output tokens: {}", output);
            }
            if let Some(cache_read) = group.cache_read_tokens {
                println!("    Cache read tokens: {}", cache_read);
            }
            if let Some(cache_write) = group.cache_write_tokens {
                println!("    Cache write tokens: {}", cache_write);
            }
            if let Some(total) = group.total_tokens {
                println!("    Total tokens: {}", total);
            }
            if let Some(count) = group.requests_count {
                println!("    Requests count: {}", count);
            }
            if let Some(tokens) = group.tokens_per_success {
                println!("    Tokens per success: {:.2}", tokens);
            }
            if let Some(requests) = group.requests_per_success {
                println!("    Requests per success: {:.2}", requests);
            }
        }
    }
    Ok(())
}

pub fn build_summary(
    cfg: &config::GahConfig,
    since: &str,
    profile: Option<&str>,
    group_by: super::GroupBy,
) -> Result<SummaryData> {
    let cutoff = parse_since(since)?;
    let mut entries = read_entries(cfg)?;
    if let Some(profile) = profile {
        entries.retain(|entry| entry.profile == profile);
    }
    entries.retain(|entry| entry.timestamp >= cutoff);

    let mut by_mode: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_backend: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_requested_backend: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_failure_class: BTreeMap<String, usize> = BTreeMap::new();
    let mut success = 0usize;
    let mut failed = 0usize;
    let mut fallback = 0usize;
    let mut validation_pass = 0usize;
    let mut push_success = 0usize;
    let mut mr_count = 0usize;
    let mut human_required_count = 0usize;
    let mut duration_total = 0.0f64;
    let mut duration_count = 0usize;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cache_read_tokens = 0u64;
    let mut cache_write_tokens = 0u64;
    let mut total_tokens = 0u64;
    let mut requests_count = 0u64;
    let mut estimated_cost = 0.0f64;
    let mut actual_cost = 0.0f64;
    // Issue #240: attempt counters may be unknown (pre-tracking). Sum known
    // values and count unknowns separately rather than coercing to 0.
    let mut attempts_started_sum = 0u32;
    let mut attempts_completed_sum = 0u32;
    let mut attempts_started_seen = false;
    let mut attempts_completed_seen = false;
    let mut attempts_started_unknown = 0usize;
    let mut attempts_completed_unknown = 0usize;
    // Track whether we've actually observed each metric (None != 0)
    let mut input_tokens_seen = false;
    let mut output_tokens_seen = false;
    let mut cache_read_tokens_seen = false;
    let mut cache_write_tokens_seen = false;
    let mut total_tokens_seen = false;
    let mut requests_count_seen = false;
    let mut estimated_cost_seen = false;
    let mut actual_cost_seen = false;
    for entry in &entries {
        *by_mode.entry(entry.mode.clone()).or_default() += 1;
        *by_backend
            .entry(crate::config::canonical_backend_name(&entry.effective_backend).to_string())
            .or_default() += 1;
        *by_requested_backend
            .entry(crate::config::canonical_backend_name(&entry.requested_backend).to_string())
            .or_default() += 1;
        if let Some(model) = &entry.effective_model {
            *by_model.entry(model.clone()).or_default() += 1;
        }
        if let Some(class) = &entry.failure_class {
            *by_failure_class.entry(class.clone()).or_default() += 1;
        }
        if entry.error_summary.is_some() {
            failed += 1;
        } else {
            success += 1;
        }
        if entry.fallback_used {
            fallback += 1;
        }
        if matches!(
            entry.validation_result.as_deref(),
            Some("passed") | Some("APPROVE")
        ) {
            validation_pass += 1;
        }
        if entry.push_succeeded {
            push_success += 1;
        }
        if entry.mr_created {
            mr_count += 1;
        }
        if entry.human_required {
            human_required_count += 1;
        }
        if let Some(duration) = entry.duration_seconds {
            duration_total += duration;
            duration_count += 1;
        }
        // Issue #240: unknown attempt counters must stay unknown.
        match entry.attempts_started {
            Some(n) => {
                attempts_started_sum += n;
                attempts_started_seen = true;
            }
            None => attempts_started_unknown += 1,
        }
        match entry.attempts_completed {
            Some(n) => {
                attempts_completed_sum += n;
                attempts_completed_seen = true;
            }
            None => attempts_completed_unknown += 1,
        }
        for observed in canonical_usage_observations(entry) {
            if let Some(tokens) = observed.usage.input_tokens {
                input_tokens += tokens;
                input_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.output_tokens {
                output_tokens += tokens;
                output_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.cache_read_tokens {
                cache_read_tokens += tokens;
                cache_read_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.cache_write_tokens {
                cache_write_tokens += tokens;
                cache_write_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.total_tokens {
                total_tokens += tokens;
                total_tokens_seen = true;
            }
            if let Some(count) = observed.usage.requests_count {
                requests_count += count;
                requests_count_seen = true;
            }
            if let Some(cost) = observed.usage.estimated_cost_usd {
                estimated_cost += cost;
                estimated_cost_seen = true;
            }
            if let Some(cost) = observed.usage.actual_cost_usd {
                actual_cost += cost;
                actual_cost_seen = true;
            }
        }
    }

    let last_run = entries.last().map(|last| {
        format!(
            "{} {} {} {}",
            last.timestamp, last.profile, last.mode, last.effective_backend
        )
    });

    // TICKET-125: Build grouped data if requested
    let grouped_by_backend = if group_by == super::GroupBy::Backend {
        build_grouped_summary(
            &entries,
            |entry| crate::config::canonical_backend_name(&entry.effective_backend).to_string(),
            |observed| crate::config::canonical_backend_name(observed.backend).to_string(),
            |backend, _model| crate::config::canonical_backend_name(backend).to_string(),
            true,
        )
    } else {
        None
    };

    let grouped_by_model = if group_by == super::GroupBy::Model {
        build_grouped_summary(
            &entries,
            |entry| {
                entry
                    .effective_model
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_MODEL_LABEL.to_string())
            },
            |observed| {
                observed
                    .model
                    .map(str::to_string)
                    .unwrap_or_else(|| UNKNOWN_MODEL_LABEL.to_string())
            },
            |_backend, model| {
                model
                    .map(str::to_string)
                    .unwrap_or_else(|| UNKNOWN_MODEL_LABEL.to_string())
            },
            false,
        )
    } else {
        None
    };

    Ok(SummaryData {
        ledger_path: cfg.defaults.ledger_path().display().to_string(),
        entries: entries.len(),
        success,
        failed,
        by_mode,
        by_requested_backend,
        by_backend,
        by_model,
        by_failure_class,
        fallback_count: fallback,
        validation_pass,
        push_success,
        mr_count,
        human_required_count,
        attempts_started: attempts_started_seen.then_some(attempts_started_sum),
        attempts_completed: attempts_completed_seen.then_some(attempts_completed_sum),
        attempts_started_unknown,
        attempts_completed_unknown,
        average_duration_seconds: (duration_count > 0)
            .then_some(duration_total / duration_count as f64),
        usage_input_tokens: input_tokens_seen.then_some(input_tokens),
        usage_output_tokens: output_tokens_seen.then_some(output_tokens),
        usage_cache_read_tokens: cache_read_tokens_seen.then_some(cache_read_tokens),
        usage_cache_write_tokens: cache_write_tokens_seen.then_some(cache_write_tokens),
        usage_total_tokens: total_tokens_seen.then_some(total_tokens),
        usage_requests_count: requests_count_seen.then_some(requests_count),
        estimated_cost_usd: estimated_cost_seen.then_some(estimated_cost),
        actual_cost_usd: actual_cost_seen.then_some(actual_cost),
        last_run,
        grouped_by_backend,
        grouped_by_model,
    })
}

/// TICKET-125: Build grouped summary data for a specific grouping key
pub fn build_grouped_summary<F, U, A>(
    entries: &[LedgerEntry],
    entry_group_key_fn: F,
    usage_group_key_fn: U,
    attempt_group_key_fn: A,
    merge_account_quota: bool,
) -> Option<Vec<GroupSummary>>
where
    F: Fn(&LedgerEntry) -> String,
    U: Fn(UsageObservation<'_>) -> String,
    A: Fn(&str, Option<&str>) -> String,
{
    build_grouped_summary_with_account_quota(
        entries,
        entry_group_key_fn,
        usage_group_key_fn,
        attempt_group_key_fn,
        merge_account_quota,
        &crate::quota_store::load_account_observations(),
    )
}

/// Like [`build_grouped_summary`] but with the account-level quota
pub fn build_grouped_summary_with_account_quota<F, U, A>(
    entries: &[LedgerEntry],
    entry_group_key_fn: F,
    usage_group_key_fn: U,
    attempt_group_key_fn: A,
    merge_account_quota: bool,
    account_quota_observations: &[crate::quota_store::QuotaObservationRecord],
) -> Option<Vec<GroupSummary>>
where
    F: Fn(&LedgerEntry) -> String,
    U: Fn(UsageObservation<'_>) -> String,
    A: Fn(&str, Option<&str>) -> String,
{
    if entries.is_empty() {
        return None;
    }

    let mut groups: BTreeMap<String, Vec<&LedgerEntry>> = BTreeMap::new();
    let mut usage_groups: BTreeMap<String, Vec<UsageObservation<'_>>> = BTreeMap::new();
    let mut attempt_counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        let key = entry_group_key_fn(entry);
        groups.entry(key).or_default().push(entry);
        for observed in canonical_usage_observations(entry) {
            usage_groups
                .entry(usage_group_key_fn(observed.clone()))
                .or_default()
                .push(observed);
        }
        for (backend, model) in execution_identities(entry) {
            *attempt_counts
                .entry(attempt_group_key_fn(backend.as_str(), model.as_deref()))
                .or_default() += 1;
        }
    }

    let mut summaries = Vec::new();
    // #166 / #151 cross-cutting: durable account-level quota observations
    // (e.g. from `codex status --json`) are kept in a separate store
    // from per-attempt usage. They are injected by the caller; merging
    // into each group is scoped so it can never fabricate data where none
    // exists.
    let all_group_keys: std::collections::BTreeSet<String> = groups
        .keys()
        .chain(usage_groups.keys())
        .chain(attempt_counts.keys())
        .cloned()
        .collect();
    for group_key in all_group_keys {
        let group_entries = groups.remove(&group_key).unwrap_or_default();
        let group_usage = usage_groups.remove(&group_key).unwrap_or_default();
        let group_entry_count = group_entries.len();
        let attempts = attempt_counts.remove(&group_key).unwrap_or(0);
        // Issue #240: attempt counters may be unknown (pre-tracking).
        let mut attempts_started_sum = 0u32;
        let mut attempts_completed_sum = 0u32;
        let mut attempts_started_seen = false;
        let mut attempts_completed_seen = false;
        let mut attempts_started_unknown = 0usize;
        let mut attempts_completed_unknown = 0usize;
        let mut validation_pass = 0usize;
        let mut review_verdict_distribution: BTreeMap<String, usize> = BTreeMap::new();
        let mut total_cost_usd = 0.0f64;
        let mut cost_seen = false;
        let mut actual_cost_total = 0.0f64;
        let mut estimated_cost_total = 0.0f64;
        let mut actual_cost_seen = false;
        let mut estimated_cost_seen = false;
        let mut strong_approve_count = 0usize;
        let mut total_duration = 0.0f64;
        let mut duration_count = 0usize;
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut cache_read_tokens = 0u64;
        let mut cache_write_tokens = 0u64;
        let mut total_tokens = 0u64;
        let mut requests_count = 0u64;
        let mut input_tokens_seen = false;
        let mut output_tokens_seen = false;
        let mut cache_read_tokens_seen = false;
        let mut cache_write_tokens_seen = false;
        let mut total_tokens_seen = false;
        let mut requests_count_seen = false;
        let mut quota_observations: BTreeMap<
            (String, Option<String>, Option<String>),
            GroupQuotaObservation,
        > = BTreeMap::new();

        for entry in &group_entries {
            // Count validation passes
            if matches!(
                entry.validation_result.as_deref(),
                Some("passed") | Some("APPROVE")
            ) {
                validation_pass += 1;
            }

            // Count review verdict distribution
            if let Some(verdict) = &entry.review_verdict {
                *review_verdict_distribution
                    .entry(verdict.clone())
                    .or_default() += 1;
                // Issue #214: key the per-tier cost metric on the
                // persisted `reviewer_tier` field rather than sniffing
                // verdict text. The verdict is always `APPROVE` now (the
                // STRONG/WEAK self-reported split was removed in PR #213),
                // so it can no longer proxy tier -- `reviewer_tier`
                // (derived from routing config) is the real authority
                // signal and is what `cost_per_approve_strong` counts.
                if verdict == "APPROVE" && entry.reviewer_tier.as_deref() == Some("strong") {
                    strong_approve_count += 1;
                }
            }

            // Sum up durations
            if let Some(duration) = entry.duration_seconds {
                total_duration += duration;
                duration_count += 1;
            }

            // Issue #240: unknown attempt counters must stay unknown.
            match entry.attempts_started {
                Some(n) => {
                    attempts_started_sum += n;
                    attempts_started_seen = true;
                }
                None => attempts_started_unknown += 1,
            }
            match entry.attempts_completed {
                Some(n) => {
                    attempts_completed_sum += n;
                    attempts_completed_seen = true;
                }
                None => attempts_completed_unknown += 1,
            }
        }

        for observed in group_usage {
            if let Some(tokens) = observed.usage.input_tokens {
                input_tokens += tokens;
                input_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.output_tokens {
                output_tokens += tokens;
                output_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.cache_read_tokens {
                cache_read_tokens += tokens;
                cache_read_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.cache_write_tokens {
                cache_write_tokens += tokens;
                cache_write_tokens_seen = true;
            }
            if let Some(tokens) = observed.usage.total_tokens {
                total_tokens += tokens;
                total_tokens_seen = true;
            }
            if let Some(count) = observed.usage.requests_count {
                requests_count += count;
                requests_count_seen = true;
            }
            if let Some(cost) = observed.usage.actual_cost_usd {
                actual_cost_total += cost;
                total_cost_usd += cost;
                actual_cost_seen = true;
                cost_seen = true;
            }
            if let Some(cost) = observed.usage.estimated_cost_usd {
                estimated_cost_total += cost;
                if observed.usage.actual_cost_usd.is_none() {
                    total_cost_usd += cost;
                    cost_seen = true;
                }
                estimated_cost_seen = true;
            }
            if observed.usage.quota_window.is_some()
                || observed.usage.quota_used_percent.is_some()
                || observed.usage.quota_remaining_percent.is_some()
                || observed.usage.quota_reset_at.is_some()
            {
                let key = (
                    observed.backend.to_string(),
                    observed.model.map(str::to_string),
                    observed.usage.quota_window.clone(),
                );
                let candidate = GroupQuotaObservation {
                    backend: observed.backend.to_string(),
                    model: observed.model.map(str::to_string),
                    quota_window: observed.usage.quota_window.clone(),
                    quota_used_percent: observed.usage.quota_used_percent,
                    quota_remaining_percent: observed.usage.quota_remaining_percent,
                    quota_reset_at: observed.usage.quota_reset_at.clone(),
                    observed_at: observed.usage.observed_at.clone(),
                    usage_source: observed.usage.usage_source.clone(),
                };
                let replace = is_timestamp_earlier(
                    &quota_observations
                        .get(&key)
                        .and_then(|e| e.observed_at.as_ref()),
                    &candidate.observed_at.as_ref(),
                );
                if replace {
                    quota_observations.insert(key, candidate);
                }
            }
        }

        // Merge account-level quota observations if requested
        if merge_account_quota {
            for account_obs in account_quota_observations {
                if account_obs.backend == group_key
                    || account_obs
                        .model
                        .as_deref()
                        .unwrap_or("")
                        .contains(&group_key)
                {
                    let key = (
                        account_obs.backend.clone(),
                        account_obs.model.clone(),
                        account_obs.quota_window.clone(),
                    );
                    let candidate = GroupQuotaObservation {
                        backend: account_obs.backend.clone(),
                        model: account_obs.model.clone(),
                        quota_window: account_obs.quota_window.clone(),
                        quota_used_percent: account_obs.quota_used_percent,
                        quota_remaining_percent: account_obs.quota_remaining_percent,
                        quota_reset_at: account_obs.quota_reset_at.clone(),
                        observed_at: account_obs.observed_at.clone(),
                        usage_source: account_obs.usage_source.clone(),
                    };
                    let replace = is_timestamp_earlier(
                        &quota_observations
                            .get(&key)
                            .and_then(|e| e.observed_at.as_ref()),
                        &candidate.observed_at.as_ref(),
                    );
                    if replace {
                        quota_observations.insert(key, candidate);
                    }
                }
            }
        }

        let mut quota_observations_vec: Vec<GroupQuotaObservation> =
            quota_observations.into_values().collect();
        quota_observations_vec.sort_by(|a, b| {
            let a_time = a.observed_at.as_deref().unwrap_or("");
            let b_time = b.observed_at.as_deref().unwrap_or("");
            b_time.cmp(a_time) // newest first
        });

        let success_rate = if group_entry_count > 0 {
            Some(validation_pass as f64 / group_entry_count as f64)
        } else {
            None
        };

        let average_cost_usd = if cost_seen && group_entry_count > 0 {
            Some(total_cost_usd / group_entry_count as f64)
        } else {
            None
        };

        let average_duration_seconds = if duration_count > 0 {
            Some(total_duration / duration_count as f64)
        } else {
            None
        };

        let tokens_per_success = if validation_pass > 0 {
            input_tokens_seen.then(|| input_tokens as f64 / validation_pass as f64)
        } else {
            None
        };

        let requests_per_success = if validation_pass > 0 {
            requests_count_seen.then(|| requests_count as f64 / validation_pass as f64)
        } else {
            None
        };

        let cost_per_approve_strong = if strong_approve_count > 0 && actual_cost_seen {
            Some(actual_cost_total / strong_approve_count as f64)
        } else {
            None
        };

        summaries.push(GroupSummary {
            group_key,
            entries: group_entry_count,
            attempts,
            attempts_started: attempts_started_seen.then_some(attempts_started_sum),
            attempts_completed: attempts_completed_seen.then_some(attempts_completed_sum),
            attempts_started_unknown,
            attempts_completed_unknown,
            validation_pass,
            success_rate,
            review_verdict_distribution,
            total_cost_usd: cost_seen.then_some(total_cost_usd),
            actual_cost_usd: actual_cost_seen.then_some(actual_cost_total),
            estimated_cost_usd: estimated_cost_seen.then_some(estimated_cost_total),
            average_cost_usd,
            average_duration_seconds,
            input_tokens: input_tokens_seen.then_some(input_tokens),
            output_tokens: output_tokens_seen.then_some(output_tokens),
            cache_read_tokens: cache_read_tokens_seen.then_some(cache_read_tokens),
            cache_write_tokens: cache_write_tokens_seen.then_some(cache_write_tokens),
            total_tokens: total_tokens_seen.then_some(total_tokens),
            requests_count: requests_count_seen.then_some(requests_count),
            tokens_per_success,
            requests_per_success,
            cost_per_approve_strong,
            quota_observations: quota_observations_vec,
        });
    }

    Some(summaries)
}

fn is_timestamp_earlier<T: AsRef<str>>(a: &Option<T>, b: &Option<T>) -> bool {
    use time::format_description::well_known::Rfc3339;
    match (a, b) {
        (None, Some(_)) => true,  // Missing timestamp is considered earliest
        (Some(_), None) => false, // Missing timestamp is considered latest
        (None, None) => false,
        (Some(a_str), Some(b_str)) => {
            // Parse RFC3339 timestamps, falling back to string comparison if parsing fails
            match (
                OffsetDateTime::parse(a_str.as_ref(), &Rfc3339),
                OffsetDateTime::parse(b_str.as_ref(), &Rfc3339),
            ) {
                (Ok(a_dt), Ok(b_dt)) => a_dt < b_dt,
                _ => a_str.as_ref() < b_str.as_ref(), // Fallback to lexicographic comparison
            }
        }
    }
}

fn usage_has_observation(usage: &LedgerUsage) -> bool {
    usage.usage_source.is_some()
        || usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.cache_read_tokens.is_some()
        || usage.cache_write_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.requests_count.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.actual_cost_usd.is_some()
        || usage.quota_window.is_some()
        || usage.quota_used_percent.is_some()
        || usage.quota_remaining_percent.is_some()
        || usage.quota_reset_at.is_some()
}

#[derive(Clone)]
pub struct UsageObservation<'a> {
    pub backend: &'a str,
    pub model: Option<&'a str>,
    pub usage: &'a LedgerUsage,
}

fn canonical_usage_observations(entry: &LedgerEntry) -> Vec<UsageObservation<'_>> {
    let attempt_usage: Vec<_> = entry
        .attempts
        .iter()
        .filter(|attempt| usage_has_observation(&attempt.usage))
        .map(|attempt| UsageObservation {
            backend: attempt.backend.as_str(),
            model: attempt.effective_model.as_deref(),
            usage: &attempt.usage,
        })
        .collect();
    if !attempt_usage.is_empty() {
        return attempt_usage;
    }
    if usage_has_observation(&entry.usage) {
        return vec![UsageObservation {
            backend: entry.effective_backend.as_str(),
            model: entry.effective_model.as_deref(),
            usage: &entry.usage,
        }];
    }
    Vec::new()
}

fn execution_identities(entry: &LedgerEntry) -> Vec<(String, Option<String>)> {
    if !entry.attempts.is_empty() {
        return entry
            .attempts
            .iter()
            .map(|attempt| (attempt.backend.clone(), attempt.effective_model.clone()))
            .collect();
    }
    vec![(
        entry.effective_backend.clone(),
        entry.effective_model.clone(),
    )]
}

fn print_counts(counts: &BTreeMap<String, usize>) {
    for (key, count) in counts {
        println!("  {:<16} {}", key, count);
    }
}

pub(crate) fn parse_since(input: &str) -> Result<String> {
    let now = OffsetDateTime::now_utc();
    let trimmed = input.trim();
    if let Some(days) = trimmed.strip_suffix('d') {
        let days = days.parse::<i64>()?;
        return Ok(
            (now - Duration::days(days)).format(&time::format_description::well_known::Rfc3339)?
        );
    }
    if let Some(hours) = trimmed.strip_suffix('h') {
        let hours = hours.parse::<i64>()?;
        return Ok((now - Duration::hours(hours))
            .format(&time::format_description::well_known::Rfc3339)?);
    }
    anyhow::bail!(
        "invalid --since value '{}'; use forms like 7d or 24h",
        input
    );
}

pub const UNKNOWN_MODEL_LABEL: &str = "(unknown model)";
