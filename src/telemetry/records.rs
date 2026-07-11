//! Telemetry Record Definitions
//!
//! Contains all the data structures for telemetry export.

use serde::{Deserialize, Serialize};

/// Current schema version for exported telemetry records
pub const SCHEMA_VERSION: u32 = 2;

/// Record types for telemetry data (used for enum tags)
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    /// Per-attempt usage and execution data
    AttemptUsage,
    /// Quota observation data
    QuotaObservation,
    /// Task outcome data
    TaskOutcome,
    /// Review outcome data
    ReviewOutcome,
}

/// Canonical telemetry record with schema versioning
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TelemetryRecord {
    /// Schema version for forward/backward compatibility
    pub schema_version: u32,
    /// Unique identifier for this record
    pub record_id: String,
    /// Timestamp when this record was created/exported
    pub exported_at: String,
    /// The original timestamp from the source data
    pub observed_at: String,
}

/// Attempt usage telemetry record
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AttemptUsageRecord {
    #[serde(flatten)]
    pub base: TelemetryRecord,

    /// Profile identifier
    pub profile: String,
    /// Repository identifier
    pub repo_id: String,
    /// Repository name
    pub repo: String,
    /// Provider (github, gitlab, etc.)
    pub provider: String,

    /// Work identifier
    pub work_id: Option<String>,
    /// Target summary/description
    pub target_summary: Option<String>,
    /// Mode of operation (fix, improve, review, etc.)
    pub mode: String,

    /// Attempt identifier within the dispatch
    pub attempt_number: u32,
    /// Backend used for this attempt
    pub backend: String,
    /// Effective backend (may differ from requested due to fallback)
    pub effective_backend: String,
    /// Requested backend
    pub requested_backend: String,
    /// Effective model used
    pub effective_model: Option<String>,
    /// Requested model
    pub requested_model: Option<String>,

    /// Exit code from the attempt
    pub exit_code: Option<i32>,
    /// Duration in seconds
    pub duration_seconds: Option<f64>,
    /// Validation result
    pub validation_result: Option<String>,
    /// Failure class
    pub failure_class: Option<String>,
    /// Failure stage
    pub failure_stage: Option<String>,

    /// Whether this was a fallback attempt
    pub fallback_used: bool,
    /// Whether human intervention was required
    pub human_required: bool,
    /// Routing reason
    pub routing_reason: Option<String>,

    /// Usage source (where the usage data came from)
    pub usage_source: Option<String>,
    /// Explicit accounting classification: quota_backed, api_key_backed,
    /// local_unmetered, or unknown.
    pub usage_classification: Option<String>,
    /// Safe logical backend instance/account attribution.
    pub backend_instance: Option<String>,
    pub model_provider: Option<String>,
    pub account_label: Option<String>,
    pub pricing_source: Option<String>,
    pub pricing_version: Option<String>,
    pub cost_unknown_reason: Option<String>,
    /// Input tokens consumed
    pub input_tokens: Option<u64>,
    /// Output tokens produced
    pub output_tokens: Option<u64>,
    /// Cache read tokens
    pub cache_read_tokens: Option<u64>,
    /// Cache write tokens
    pub cache_write_tokens: Option<u64>,
    /// Total tokens
    pub total_tokens: Option<u64>,
    /// Number of requests made
    pub requests_count: Option<u64>,
    /// Estimated cost in USD
    pub estimated_cost_usd: Option<f64>,
    /// Actual cost in USD
    pub actual_cost_usd: Option<f64>,

    /// Quota window identifier
    pub quota_window: Option<String>,
    /// Quota used percentage
    pub quota_used_percent: Option<f64>,
    /// Quota remaining percentage
    pub quota_remaining_percent: Option<f64>,
    /// When quota resets
    pub quota_reset_at: Option<String>,
}

/// Quota observation telemetry record
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct QuotaObservationRecord {
    #[serde(flatten)]
    pub base: TelemetryRecord,

    /// Profile identifier
    pub profile: String,
    /// Repository identifier
    pub repo_id: String,
    /// Repository name
    pub repo: String,
    /// Provider (github, gitlab, etc.)
    pub provider: String,

    /// Work identifier (if associated with specific work)
    pub work_id: Option<String>,

    /// Backend being observed
    pub backend: String,
    /// Effective backend
    pub effective_backend: String,
    /// Model being observed
    pub model: Option<String>,
    /// Effective model
    pub effective_model: Option<String>,

    /// Account scope (if known)
    pub account_scope: Option<String>,
    /// Quota pool identifier (if known)
    pub quota_pool: Option<String>,
    /// Quota window identifier
    pub quota_window: String,
    /// Quota used percentage
    pub quota_used_percent: Option<f64>,
    /// Quota remaining percentage
    pub quota_remaining_percent: Option<f64>,
    /// When quota resets
    pub quota_reset_at: Option<String>,
    /// Observation source (where this data came from)
    pub observation_source: String,
}

/// Task outcome telemetry record
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TaskOutcomeRecord {
    #[serde(flatten)]
    pub base: TelemetryRecord,

    /// Profile identifier
    pub profile: String,
    /// Repository identifier
    pub repo_id: String,
    /// Repository name
    pub repo: String,
    /// Provider (github, gitlab, etc.)
    pub provider: String,

    /// Work identifier
    pub work_id: String,
    /// Target summary/description
    pub target_summary: Option<String>,
    /// Mode of operation
    pub mode: String,
    /// Branch being worked on
    pub branch: Option<String>,

    /// Dispatch reason
    pub dispatch_reason: Option<String>,
    /// Number of attempts started
    pub attempts_started: u32,
    /// Number of attempts completed
    pub attempts_completed: u32,
    /// Total duration in seconds
    pub duration_seconds: Option<f64>,
    /// Backend exit code
    pub backend_exit_code: Option<i32>,
    /// Validation result
    pub validation_result: Option<String>,
    /// Review verdict
    pub review_verdict: Option<String>,
    /// Review confidence
    pub review_confidence: Option<String>,
    /// Reviewer backend
    pub reviewer_backend: Option<String>,
    /// Reviewer model
    pub reviewer_model: Option<String>,

    /// Commit attempted
    pub commit_attempted: bool,
    /// Commit created
    pub commit_created: bool,
    /// Push attempted
    pub push_attempted: bool,
    /// Push succeeded
    pub push_succeeded: bool,
    /// MR attempted
    pub mr_attempted: bool,
    /// MR created
    pub mr_created: bool,
    /// MR URL
    pub mr_url: Option<String>,

    /// Files changed
    pub files_changed: Option<u32>,
    /// Insertions
    pub insertions: Option<u32>,
    /// Deletions
    pub deletions: Option<u32>,

    /// Failure class
    pub failure_class: Option<String>,
    /// Failure stage
    pub failure_stage: Option<String>,
    /// Error summary
    pub error_summary: Option<String>,

    /// Aggregate usage data
    pub usage_source: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub requests_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,

    /// Final outcome (derived from available data)
    pub final_outcome: Option<String>,
    /// Merge status (if applicable)
    pub merge_status: Option<String>,
}

/// Review outcome telemetry record
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ReviewOutcomeRecord {
    #[serde(flatten)]
    pub base: TelemetryRecord,

    /// Profile identifier
    pub profile: String,
    /// Repository identifier
    pub repo_id: String,
    /// Repository name
    pub repo: String,
    /// Provider (github, gitlab, etc.)
    pub provider: String,

    /// Work identifier being reviewed
    pub work_id: String,
    /// Branch being reviewed
    pub branch: Option<String>,
    /// MR URL
    pub mr_url: Option<String>,

    /// Review verdict
    pub review_verdict: String,
    /// Review confidence
    pub review_confidence: String,
    /// Reviewer backend
    pub reviewer_backend: String,
    /// Reviewer model
    pub reviewer_model: Option<String>,

    /// Duration of review in seconds
    pub duration_seconds: Option<f64>,
    /// Timestamp when review was completed
    pub review_completed_at: String,

    /// Backend/model that created the work being reviewed
    pub implementation_backend: Option<String>,
    /// Model that created the work being reviewed
    pub implementation_model: Option<String>,
}

/// Exported telemetry record (enum of all possible record types)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "record_type", content = "data")]
pub enum ExportedTelemetryRecord {
    #[serde(rename = "attempt_usage")]
    AttemptUsage(AttemptUsageRecord),
    #[serde(rename = "quota_observation")]
    QuotaObservation(QuotaObservationRecord),
    #[serde(rename = "task_outcome")]
    TaskOutcome(TaskOutcomeRecord),
    #[serde(rename = "review_outcome")]
    ReviewOutcome(ReviewOutcomeRecord),
}

impl ExportedTelemetryRecord {
    pub fn get_id(&self) -> String {
        match self {
            ExportedTelemetryRecord::AttemptUsage(record) => record.base.record_id.clone(),
            ExportedTelemetryRecord::QuotaObservation(record) => record.base.record_id.clone(),
            ExportedTelemetryRecord::TaskOutcome(record) => record.base.record_id.clone(),
            ExportedTelemetryRecord::ReviewOutcome(record) => record.base.record_id.clone(),
        }
    }

    pub fn get_observed_at(&self) -> String {
        match self {
            ExportedTelemetryRecord::AttemptUsage(record) => record.base.observed_at.clone(),
            ExportedTelemetryRecord::QuotaObservation(record) => record.base.observed_at.clone(),
            ExportedTelemetryRecord::TaskOutcome(record) => record.base.observed_at.clone(),
            ExportedTelemetryRecord::ReviewOutcome(record) => record.base.observed_at.clone(),
        }
    }

    #[allow(dead_code)]
    pub fn get_record_type(&self) -> RecordType {
        match self {
            ExportedTelemetryRecord::AttemptUsage(_) => RecordType::AttemptUsage,
            ExportedTelemetryRecord::QuotaObservation(_) => RecordType::QuotaObservation,
            ExportedTelemetryRecord::TaskOutcome(_) => RecordType::TaskOutcome,
            ExportedTelemetryRecord::ReviewOutcome(_) => RecordType::ReviewOutcome,
        }
    }
}

/// Counts of different record types for manifests
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RecordTypeCounts {
    pub attempt_usage: usize,
    pub quota_observation: usize,
    pub task_outcome: usize,
    pub review_outcome: usize,
}

/// Manifest for a daily partition
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartitionManifest {
    /// Partition identifier (e.g., "2026-07-10.jsonl")
    pub partition: String,
    /// Total number of records in this partition
    pub records: usize,
    /// First timestamp in the partition
    pub first_observed_at: String,
    /// Last timestamp in the partition
    pub last_observed_at: String,
    /// Schema version
    pub schema_version: u32,
    /// SHA-256 digest of the partition file (optional)
    pub sha256: Option<String>,
    /// Counts by record type
    pub record_type_counts: RecordTypeCounts,
}

/// Partition key for organizing telemetry data by date
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct PartitionKey {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl PartitionKey {
    pub fn from_date_str(date_str: &str) -> Option<Self> {
        use time::format_description::well_known::Rfc3339;
        use time::OffsetDateTime;

        // Try to parse various date formats
        if let Ok(parsed) = OffsetDateTime::parse(date_str, &Rfc3339) {
            return Some(Self {
                year: parsed.year(),
                month: parsed.month() as u32,
                day: parsed.day() as u32,
            });
        }

        // Try YYYY-MM-DD format
        let parts: Vec<&str> = date_str.split('-').collect();
        if parts.len() == 3 {
            if let (Ok(year), Ok(month), Ok(day)) = (
                parts[0].parse::<i32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            ) {
                return Some(Self { year, month, day });
            }
        }

        None
    }

    pub fn to_path_string(self) -> String {
        format!("{:04}/{:02}/{:02}", self.year, self.month, self.day)
    }

    pub fn to_date_string(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Generate partition key from timestamp
pub fn partition_key_from_timestamp(timestamp: &str) -> PartitionKey {
    PartitionKey::from_date_str(timestamp).unwrap_or_else(|| {
        use time::OffsetDateTime;
        // Default to today if parsing fails
        let now = OffsetDateTime::now_utc();
        PartitionKey {
            year: now.year(),
            month: now.month() as u32,
            day: now.day() as u32,
        }
    })
}

/// Generate a deterministic record ID for attempt usage
pub fn generate_attempt_usage_id(
    entry_timestamp: &str,
    work_id: Option<&str>,
    attempt_number: u32,
    backend: &str,
    effective_model: Option<&str>,
) -> String {
    let work_part = work_id.map(|w| format!("w:{}", w)).unwrap_or_default();
    let model_part = effective_model
        .map(|m| format!("m:{}", m))
        .unwrap_or_else(|| backend.to_string());
    format!(
        "attempt_usage:{}:a{}:{}:{}:{}",
        entry_timestamp, attempt_number, backend, model_part, work_part
    )
}

/// Generate a deterministic record ID for quota observation
pub fn generate_quota_observation_id(
    observed_at: &str,
    backend: &str,
    model: Option<&str>,
    quota_window: &str,
) -> String {
    let model_part = model.map(|m| format!("m:{}", m)).unwrap_or_default();
    format!(
        "quota_obs:{}:{}:{}:{}",
        observed_at, backend, model_part, quota_window
    )
}

/// Generate a deterministic record ID for task outcome
pub fn generate_task_outcome_id(entry_timestamp: &str, work_id: Option<&str>) -> String {
    let work_part = work_id
        .map(|w| format!("w:{}", w))
        .unwrap_or_else(|| entry_timestamp.to_string());
    format!("task_outcome:{}:{}", entry_timestamp, work_part)
}

/// Generate a deterministic record ID for review outcome
pub fn generate_review_outcome_id(
    entry_timestamp: &str,
    work_id: &str,
    review_verdict: &str,
    review_completed_at: &str,
) -> String {
    format!(
        "review_outcome:{}:w:{}:v:{}:{}",
        entry_timestamp, work_id, review_verdict, review_completed_at
    )
}
