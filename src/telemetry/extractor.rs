//! Telemetry Extractor
//!
//! Extracts telemetry records from ledger entries.

use super::records::*;
use crate::ledger::LedgerEntry;

/// Extract attempt usage records from a ledger entry
pub fn extract_attempt_usage_records(
    entry: &LedgerEntry,
    exported_at: &str,
) -> Vec<AttemptUsageRecord> {
    let mut records = Vec::new();

    for attempt in &entry.attempts {
        let observed_at = attempt
            .usage
            .observed_at
            .clone()
            .or_else(|| Some(entry.timestamp.clone()))
            .unwrap_or_else(|| exported_at.to_string());

        let base = TelemetryRecord {
            schema_version: SCHEMA_VERSION,
            record_id: generate_attempt_usage_id(
                &entry.timestamp,
                entry.work_id.as_deref(),
                attempt.attempt_number,
                &attempt.backend,
                attempt.effective_model.as_deref(),
            ),
            exported_at: exported_at.to_string(),
            observed_at: observed_at.clone(),
        };

        let record = AttemptUsageRecord {
            base,
            profile: entry.profile.clone(),
            repo_id: entry.repo_id.clone(),
            repo: entry.repo.clone(),
            provider: entry.provider.clone(),
            work_id: entry.work_id.clone(),
            target_summary: entry.target_summary.clone(),
            mode: entry.mode.clone(),
            attempt_number: attempt.attempt_number,
            backend: attempt.backend.clone(),
            effective_backend: attempt.backend.clone(),
            requested_backend: entry.requested_backend.clone(),
            effective_model: attempt.effective_model.clone(),
            requested_model: entry.requested_model.clone(),
            exit_code: attempt.exit_code,
            duration_seconds: attempt.duration_seconds,
            validation_result: attempt.validation_result.clone(),
            failure_class: attempt.failure_class.clone(),
            failure_stage: attempt.failure_stage.clone(),
            fallback_used: entry.fallback_used,
            human_required: entry.human_required,
            routing_reason: entry.routing_reason.clone(),
            usage_source: attempt.usage.usage_source.clone(),
            usage_classification: attempt
                .usage
                .usage_classification
                .clone()
                .or_else(|| entry.usage.usage_classification.clone()),
            backend_instance: attempt
                .usage
                .backend_instance
                .clone()
                .or_else(|| Some(attempt.backend.clone())),
            model_provider: attempt
                .usage
                .provider
                .clone()
                .or_else(|| entry.usage.provider.clone()),
            account_label: attempt
                .usage
                .account_label
                .clone()
                .or_else(|| entry.usage.account_label.clone()),
            pricing_source: attempt
                .usage
                .pricing_source
                .clone()
                .or_else(|| entry.usage.pricing_source.clone()),
            pricing_version: attempt
                .usage
                .pricing_version
                .clone()
                .or_else(|| entry.usage.pricing_version.clone()),
            cost_unknown_reason: attempt
                .usage
                .cost_unknown_reason
                .clone()
                .or_else(|| entry.usage.cost_unknown_reason.clone()),
            input_tokens: attempt.usage.input_tokens,
            output_tokens: attempt.usage.output_tokens,
            cache_read_tokens: attempt.usage.cache_read_tokens,
            cache_write_tokens: attempt.usage.cache_write_tokens,
            total_tokens: attempt.usage.total_tokens,
            requests_count: attempt.usage.requests_count,
            estimated_cost_usd: attempt.usage.estimated_cost_usd,
            actual_cost_usd: attempt.usage.actual_cost_usd,
            quota_window: attempt.usage.quota_window.clone(),
            quota_used_percent: attempt.usage.quota_used_percent,
            quota_remaining_percent: attempt.usage.quota_remaining_percent,
            quota_reset_at: attempt.usage.quota_reset_at.clone(),
        };

        records.push(record);
    }

    records
}

/// Extract quota observation records from a ledger entry
pub fn extract_quota_observation_records(
    entry: &LedgerEntry,
    exported_at: &str,
) -> Vec<QuotaObservationRecord> {
    let mut records = Vec::new();

    // Extract from top-level entry usage if it has quota information
    if entry.usage.quota_window.is_some()
        || entry.usage.quota_used_percent.is_some()
        || entry.usage.quota_remaining_percent.is_some()
    {
        let observed_at = entry
            .usage
            .observed_at
            .clone()
            .unwrap_or_else(|| entry.timestamp.clone());

        let quota_window = entry.usage.quota_window.clone().unwrap_or_default();

        let base = TelemetryRecord {
            schema_version: SCHEMA_VERSION,
            record_id: generate_quota_observation_id(
                &observed_at,
                &entry.effective_backend,
                entry.effective_model.as_deref(),
                &quota_window,
            ),
            exported_at: exported_at.to_string(),
            observed_at: observed_at.clone(),
        };

        let record = QuotaObservationRecord {
            base,
            profile: entry.profile.clone(),
            repo_id: entry.repo_id.clone(),
            repo: entry.repo.clone(),
            provider: entry.provider.clone(),
            work_id: entry.work_id.clone(),
            backend: entry.backend.clone(),
            effective_backend: entry.effective_backend.clone(),
            model: entry.requested_model.clone(),
            effective_model: entry.effective_model.clone(),
            account_scope: entry.usage.account_label.clone(),
            quota_pool: entry
                .routing_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.selected_quota_pool.clone()),
            quota_window: quota_window.clone(),
            quota_used_percent: entry.usage.quota_used_percent,
            quota_remaining_percent: entry.usage.quota_remaining_percent,
            quota_reset_at: entry.usage.quota_reset_at.clone(),
            observation_source: entry
                .usage
                .usage_source
                .clone()
                .unwrap_or_else(|| "ledger_entry".to_string()),
        };

        records.push(record);
    }

    // Extract from individual attempt usage
    for attempt in &entry.attempts {
        if attempt.usage.quota_window.is_some()
            || attempt.usage.quota_used_percent.is_some()
            || attempt.usage.quota_remaining_percent.is_some()
        {
            let observed_at = attempt
                .usage
                .observed_at
                .clone()
                .unwrap_or_else(|| entry.timestamp.clone());

            let quota_window = attempt.usage.quota_window.clone().unwrap_or_default();

            let base = TelemetryRecord {
                schema_version: SCHEMA_VERSION,
                record_id: generate_quota_observation_id(
                    &observed_at,
                    &attempt.backend,
                    attempt.effective_model.as_deref(),
                    &quota_window,
                ),
                exported_at: exported_at.to_string(),
                observed_at: observed_at.clone(),
            };

            let record = QuotaObservationRecord {
                base,
                profile: entry.profile.clone(),
                repo_id: entry.repo_id.clone(),
                repo: entry.repo.clone(),
                provider: entry.provider.clone(),
                work_id: entry.work_id.clone(),
                backend: attempt.backend.clone(),
                effective_backend: attempt.backend.clone(),
                model: attempt.effective_model.clone(),
                effective_model: attempt.effective_model.clone(),
                account_scope: attempt
                    .usage
                    .account_label
                    .clone()
                    .or_else(|| entry.usage.account_label.clone()),
                quota_pool: entry
                    .routing_diagnostics
                    .as_ref()
                    .and_then(|diagnostics| diagnostics.selected_quota_pool.clone()),
                quota_window: quota_window.clone(),
                quota_used_percent: attempt.usage.quota_used_percent,
                quota_remaining_percent: attempt.usage.quota_remaining_percent,
                quota_reset_at: attempt.usage.quota_reset_at.clone(),
                observation_source: attempt
                    .usage
                    .usage_source
                    .clone()
                    .unwrap_or_else(|| "attempt_usage".to_string()),
            };

            records.push(record);
        }
    }

    records
}

/// Extract task outcome records from a ledger entry
pub fn extract_task_outcome_records(
    entry: &LedgerEntry,
    exported_at: &str,
) -> Vec<TaskOutcomeRecord> {
    let observed_at = entry.timestamp.clone();

    let final_outcome = determine_final_outcome(entry);
    let merge_status = determine_merge_status(entry);

    // Use work_id if available, otherwise use timestamp
    let work_id = entry
        .work_id
        .clone()
        .unwrap_or_else(|| entry.timestamp.clone());

    let base = TelemetryRecord {
        schema_version: SCHEMA_VERSION,
        record_id: generate_task_outcome_id(&entry.timestamp, entry.work_id.as_deref()),
        exported_at: exported_at.to_string(),
        observed_at: observed_at.clone(),
    };

    vec![TaskOutcomeRecord {
        base,
        profile: entry.profile.clone(),
        repo_id: entry.repo_id.clone(),
        repo: entry.repo.clone(),
        provider: entry.provider.clone(),
        work_id,
        target_summary: entry.target_summary.clone(),
        mode: entry.mode.clone(),
        branch: entry.branch.clone(),
        dispatch_reason: entry.dispatch_reason.clone(),
        attempts_started: entry.attempts_started,
        attempts_completed: entry.attempts_completed,
        duration_seconds: entry.duration_seconds,
        backend_exit_code: entry.backend_exit_code,
        validation_result: entry.validation_result.clone(),
        review_verdict: entry.review_verdict.clone(),
        review_confidence: entry.review_confidence.clone(),
        reviewer_backend: entry.reviewer_backend.clone(),
        reviewer_model: entry.reviewer_model.clone(),
        commit_attempted: entry.commit_attempted,
        commit_created: entry.commit_created,
        push_attempted: entry.push_attempted,
        push_succeeded: entry.push_succeeded,
        mr_attempted: entry.mr_attempted,
        mr_created: entry.mr_created,
        mr_url: entry.mr_url.clone(),
        files_changed: entry.files_changed,
        insertions: entry.insertions,
        deletions: entry.deletions,
        failure_class: entry.failure_class.clone(),
        failure_stage: entry.failure_stage.clone(),
        error_summary: entry.error_summary.clone(),
        usage_source: entry.usage.usage_source.clone(),
        input_tokens: entry.usage.input_tokens,
        output_tokens: entry.usage.output_tokens,
        cache_read_tokens: entry.usage.cache_read_tokens,
        cache_write_tokens: entry.usage.cache_write_tokens,
        total_tokens: entry.usage.total_tokens,
        requests_count: entry.usage.requests_count,
        estimated_cost_usd: entry.usage.estimated_cost_usd,
        actual_cost_usd: entry.usage.actual_cost_usd,
        final_outcome,
        merge_status,
    }]
}

/// Extract review outcome records from a ledger entry
pub fn extract_review_outcome_records(
    entry: &LedgerEntry,
    exported_at: &str,
) -> Vec<ReviewOutcomeRecord> {
    let mut records = Vec::new();

    // Only create review outcome records for review-mode entries with verdicts
    if entry.mode == "review" && entry.review_verdict.is_some() {
        let observed_at = entry.timestamp.clone();
        let review_completed_at = entry.timestamp.clone(); // Use entry timestamp as completion time

        let work_id = entry
            .work_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let review_verdict = entry
            .review_verdict
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let review_confidence = entry
            .review_confidence
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let reviewer_backend = entry
            .reviewer_backend
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let base = TelemetryRecord {
            schema_version: SCHEMA_VERSION,
            record_id: generate_review_outcome_id(
                &entry.timestamp,
                &work_id,
                &review_verdict,
                &review_completed_at,
            ),
            exported_at: exported_at.to_string(),
            observed_at: observed_at.clone(),
        };

        // Use the entry's own backend/model as implementation backend/model
        // In a full implementation, this would be looked up from the implementation entry
        let implementation_backend = entry.backend.clone();
        let implementation_model = entry.effective_model.clone();

        let record = ReviewOutcomeRecord {
            base,
            profile: entry.profile.clone(),
            repo_id: entry.repo_id.clone(),
            repo: entry.repo.clone(),
            provider: entry.provider.clone(),
            work_id,
            branch: entry.branch.clone(),
            mr_url: entry.mr_url.clone(),
            review_verdict,
            review_confidence,
            reviewer_backend,
            reviewer_model: entry.reviewer_model.clone(),
            duration_seconds: entry.duration_seconds,
            review_completed_at: review_completed_at.clone(),
            implementation_backend: Some(implementation_backend),
            implementation_model,
        };

        records.push(record);
    }

    records
}

/// Extract all telemetry records from a ledger entry
pub fn extract_telemetry_records(
    entry: &LedgerEntry,
    exported_at: &str,
) -> Vec<ExportedTelemetryRecord> {
    let mut all_records = Vec::new();

    // Extract attempt usage records
    for attempt_record in extract_attempt_usage_records(entry, exported_at) {
        all_records.push(ExportedTelemetryRecord::AttemptUsage(attempt_record));
    }

    // Extract quota observation records
    for quota_record in extract_quota_observation_records(entry, exported_at) {
        all_records.push(ExportedTelemetryRecord::QuotaObservation(quota_record));
    }

    // Extract task outcome records
    for task_record in extract_task_outcome_records(entry, exported_at) {
        all_records.push(ExportedTelemetryRecord::TaskOutcome(task_record));
    }

    // Extract review outcome records
    for review_record in extract_review_outcome_records(entry, exported_at) {
        all_records.push(ExportedTelemetryRecord::ReviewOutcome(review_record));
    }

    all_records
}

/// Determine final outcome from ledger entry data
pub fn determine_final_outcome(entry: &LedgerEntry) -> Option<String> {
    if entry.review_verdict.as_deref() == Some("APPROVE") {
        return Some("APPROVE".to_string());
    }
    if entry.review_verdict.as_deref() == Some("NEEDS_FIX") {
        return Some("NEEDS_FIX".to_string());
    }
    if entry.mr_created && entry.push_succeeded {
        return Some("MR_CREATED".to_string());
    }
    if entry.commit_created && entry.push_succeeded {
        return Some("COMMITTED".to_string());
    }
    if entry.validation_result.as_deref() == Some("pass") {
        return Some("VALIDATION_PASSED".to_string());
    }
    if entry.failure_class.as_deref() == Some("HumanBlocked") {
        return Some("HUMAN_BLOCKED".to_string());
    }
    if entry.failure_class.is_some() {
        return Some(format!(
            "FAILURE:{}",
            entry.failure_class.as_deref().unwrap()
        ));
    }
    if entry.backend_exit_code == Some(0) {
        return Some("SUCCESS".to_string());
    }

    None
}

/// Determine merge status from ledger entry data
pub fn determine_merge_status(entry: &LedgerEntry) -> Option<String> {
    // This would be enhanced with reconciliation data in a full implementation
    // For now, we use available fields
    if entry.mr_created {
        if entry.push_succeeded {
            return Some("MR_OPEN".to_string());
        }
        return Some("MR_CREATED".to_string());
    }

    if entry.commit_created && entry.push_succeeded {
        return Some("PUSHED".to_string());
    }

    None
}
