//! Telemetry Exporter
//!
//! Handles the export of telemetry records to the versioned repository.

use super::extractor::*;
use super::records::*;
use crate::ledger::LedgerEntry;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jsonl,
    Csv,
    Both,
}

impl std::str::FromStr for ExportFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "jsonl" => Ok(ExportFormat::Jsonl),
            "csv" => Ok(ExportFormat::Csv),
            "both" => Ok(ExportFormat::Both),
            _ => Err(format!("Unknown export format: {}", s)),
        }
    }
}

/// Configuration for telemetry export
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Path to the telemetry repository root
    pub telemetry_repo_path: PathBuf,
    /// Output format (jsonl, csv, both)
    pub format: ExportFormat,
    /// Whether to generate manifests
    pub generate_manifests: bool,
    /// Batch size for commits (None means don't commit)
    pub commit_batch_size: Option<usize>,
}

/// Telemetry exporter
pub struct TelemetryExporter {
    config: TelemetryConfig,
    exported_ids: BTreeSet<String>,
    records_exported: usize,
}

impl TelemetryExporter {
    pub fn new(config: TelemetryConfig) -> Result<Self> {
        Ok(Self {
            config,
            exported_ids: BTreeSet::new(),
            records_exported: 0,
        })
    }

    /// Load already exported record IDs from existing files
    pub fn load_exported_ids(&mut self) -> Result<()> {
        // Check if telemetry repo exists
        if !self.config.telemetry_repo_path.exists() {
            return Ok(());
        }

        // Walk through existing telemetry files and collect record IDs
        let mut collected_ids = BTreeSet::new();
        self.walk_telemetry_files(|record: &ExportedTelemetryRecord| {
            collected_ids.insert(record.get_id());
            Ok(())
        })?;
        self.exported_ids = collected_ids;

        log::debug!("Loaded {} existing record IDs", self.exported_ids.len());
        Ok(())
    }

    /// Walk through existing telemetry files and apply a function to each record
    pub fn walk_telemetry_files<F>(&self, mut callback: F) -> Result<()>
    where
        F: FnMut(&ExportedTelemetryRecord) -> Result<()>,
    {
        let raw_dir = self.config.telemetry_repo_path.join("raw");
        if !raw_dir.exists() {
            return Ok(());
        }

        // Check usage directory
        let usage_dir = raw_dir.join("usage");
        if usage_dir.exists() {
            self.walk_directory(&usage_dir, &mut callback)?;
        }

        // Check quota directory
        let quota_dir = raw_dir.join("quota");
        if quota_dir.exists() {
            self.walk_directory(&quota_dir, &mut callback)?;
        }

        // Check outcomes directory
        let outcomes_dir = raw_dir.join("outcomes");
        if outcomes_dir.exists() {
            self.walk_directory(&outcomes_dir, &mut callback)?;
        }

        Ok(())
    }

    /// Walk through a directory and apply callback to each JSONL record
    fn walk_directory<F>(&self, dir: &Path, callback: &mut F) -> Result<()>
    where
        F: FnMut(&ExportedTelemetryRecord) -> Result<()>,
    {
        if !dir.exists() || !dir.is_dir() {
            return Ok(());
        }

        let mut dirs_to_visit = vec![dir.to_path_buf()];

        while let Some(current_dir) = dirs_to_visit.pop() {
            for entry in fs::read_dir(&current_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    dirs_to_visit.push(path);
                } else if path
                    .extension()
                    .map(|e| e.to_string_lossy() == "jsonl")
                    .unwrap_or(false)
                {
                    self.process_jsonl_file(&path, callback)?;
                }
            }
        }

        Ok(())
    }

    /// Process a single JSONL file and apply callback to each record
    fn process_jsonl_file<F>(&self, path: &Path, callback: &mut F) -> Result<()>
    where
        F: FnMut(&ExportedTelemetryRecord) -> Result<()>,
    {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Reading telemetry file: {}", path.display()))?;

        for (line_num, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let record: ExportedTelemetryRecord =
                serde_json::from_str(line).with_context(|| {
                    format!(
                        "Parsing JSONL record at line {} in {}",
                        line_num + 1,
                        path.display()
                    )
                })?;

            callback(&record)?;
        }

        Ok(())
    }

    /// Export telemetry records from ledger entries
    pub fn export_from_entries(&mut self, entries: &[LedgerEntry]) -> Result<()> {
        let exported_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string());

        let mut new_records = Vec::new();

        // Extract records from all entries
        for entry in entries {
            let records = extract_telemetry_records(entry, &exported_at);
            new_records.extend(records);
        }

        // Filter out already exported records (deduplication)
        let mut records_to_export = Vec::new();
        for record in new_records {
            if !self.exported_ids.contains(&record.get_id()) {
                records_to_export.push(record);
            } else {
                log::debug!("Skipping already exported record: {}", record.get_id());
            }
        }

        if records_to_export.is_empty() {
            log::info!("No new telemetry records to export");
            return Ok(());
        }

        log::info!(
            "Exporting {} new telemetry records",
            records_to_export.len()
        );

        // Export records
        self.export_records(&records_to_export)?;

        // Update exported IDs
        for record in &records_to_export {
            self.exported_ids.insert(record.get_id());
            self.records_exported += 1;
        }

        Ok(())
    }

    /// Export a batch of telemetry records
    pub fn export_records(&self, records: &[ExportedTelemetryRecord]) -> Result<()> {
        // Group records by type and date partition
        let mut attempt_usage_by_partition: BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>> =
            BTreeMap::new();
        let mut quota_obs_by_partition: BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>> =
            BTreeMap::new();
        let mut task_outcome_by_partition: BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>> =
            BTreeMap::new();
        let mut review_outcome_by_partition: BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>> =
            BTreeMap::new();

        for record in records {
            let partition_key = partition_key_from_timestamp(&record.get_observed_at());

            match record {
                ExportedTelemetryRecord::AttemptUsage(_) => {
                    attempt_usage_by_partition
                        .entry(partition_key)
                        .or_default()
                        .push(record);
                }
                ExportedTelemetryRecord::QuotaObservation(_) => {
                    quota_obs_by_partition
                        .entry(partition_key)
                        .or_default()
                        .push(record);
                }
                ExportedTelemetryRecord::TaskOutcome(_) => {
                    task_outcome_by_partition
                        .entry(partition_key)
                        .or_default()
                        .push(record);
                }
                ExportedTelemetryRecord::ReviewOutcome(_) => {
                    review_outcome_by_partition
                        .entry(partition_key)
                        .or_default()
                        .push(record);
                }
            }
        }

        // Create telemetry repository structure if it doesn't exist
        self.ensure_repository_structure()?;

        // Export each type to its respective directory
        self.export_to_directory(&attempt_usage_by_partition, "usage")?;
        self.export_to_directory(&quota_obs_by_partition, "quota")?;
        self.export_to_directory(&task_outcome_by_partition, "outcomes")?;
        self.export_to_directory(&review_outcome_by_partition, "outcomes")?;

        // Generate manifests if enabled
        if self.config.generate_manifests {
            self.generate_manifests(
                &attempt_usage_by_partition,
                &quota_obs_by_partition,
                &task_outcome_by_partition,
                &review_outcome_by_partition,
            )?;
        }

        Ok(())
    }

    /// Ensure the telemetry repository structure exists
    fn ensure_repository_structure(&self) -> Result<()> {
        let raw_dir = self.config.telemetry_repo_path.join("raw");

        // Create raw directory
        if !raw_dir.exists() {
            fs::create_dir_all(&raw_dir).with_context(|| {
                format!("Creating telemetry raw directory: {}", raw_dir.display())
            })?;
            log::info!("Created telemetry raw directory: {}", raw_dir.display());
        }

        // Create subdirectories
        for subdir in ["usage", "quota", "outcomes"] {
            let path = raw_dir.join(subdir);
            if !path.exists() {
                fs::create_dir_all(&path).with_context(|| {
                    format!(
                        "Creating telemetry {} directory: {}",
                        subdir,
                        path.display()
                    )
                })?;
                log::info!("Created telemetry {} directory: {}", subdir, path.display());
            }
        }

        // Create exports directory
        let exports_dir = self.config.telemetry_repo_path.join("exports");
        if !exports_dir.exists() {
            fs::create_dir_all(&exports_dir).with_context(|| {
                format!(
                    "Creating telemetry exports directory: {}",
                    exports_dir.display()
                )
            })?;
            log::info!(
                "Created telemetry exports directory: {}",
                exports_dir.display()
            );
        }

        // Create manifests directory
        let manifests_dir = self.config.telemetry_repo_path.join("manifests");
        if !manifests_dir.exists() {
            fs::create_dir_all(&manifests_dir).with_context(|| {
                format!(
                    "Creating telemetry manifests directory: {}",
                    manifests_dir.display()
                )
            })?;
            log::info!(
                "Created telemetry manifests directory: {}",
                manifests_dir.display()
            );
        }

        // Create schemas directory
        let schemas_dir = self.config.telemetry_repo_path.join("schemas");
        if !schemas_dir.exists() {
            fs::create_dir_all(&schemas_dir).with_context(|| {
                format!(
                    "Creating telemetry schemas directory: {}",
                    schemas_dir.display()
                )
            })?;
            log::info!(
                "Created telemetry schemas directory: {}",
                schemas_dir.display()
            );
        }

        Ok(())
    }

    /// Export records grouped by partition to a specific directory
    fn export_to_directory(
        &self,
        records_by_partition: &BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>>,
        subdir: &str,
    ) -> Result<()> {
        let raw_dir = self.config.telemetry_repo_path.join("raw").join(subdir);

        for (partition_key, records) in records_by_partition {
            let partition_path = partition_key.to_path_string();
            let full_path = raw_dir.join(&partition_path);

            // Create partition directory if it doesn't exist
            if !full_path.parent().unwrap().exists() {
                fs::create_dir_all(full_path.parent().unwrap())?;
            }

            // Append records to the partition file
            let filename = format!("{}.jsonl", partition_key.to_date_string());
            let file_path = full_path.parent().unwrap().join(filename);

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .with_context(|| {
                    format!("Opening telemetry file for append: {}", file_path.display())
                })?;

            for record in records {
                let json = serde_json::to_string(record)
                    .with_context(|| "Serializing telemetry record")?;
                writeln!(file, "{}", json).with_context(|| {
                    format!("Writing telemetry record to {}", file_path.display())
                })?;
            }

            // Sync to ensure data is written
            file.sync_all()?;

            log::info!(
                "Exported {} records to {}",
                records.len(),
                file_path.display()
            );
        }

        Ok(())
    }

    /// Generate manifest files for exported partitions
    fn generate_manifests(
        &self,
        attempt_usage_by_partition: &BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>>,
        quota_obs_by_partition: &BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>>,
        task_outcome_by_partition: &BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>>,
        review_outcome_by_partition: &BTreeMap<PartitionKey, Vec<&ExportedTelemetryRecord>>,
    ) -> Result<()> {
        // Collect all partition keys
        let mut all_partitions: BTreeSet<PartitionKey> = BTreeSet::new();

        for key in attempt_usage_by_partition.keys() {
            all_partitions.insert(*key);
        }
        for key in quota_obs_by_partition.keys() {
            all_partitions.insert(*key);
        }
        for key in task_outcome_by_partition.keys() {
            all_partitions.insert(*key);
        }
        for key in review_outcome_by_partition.keys() {
            all_partitions.insert(*key);
        }

        for partition_key in all_partitions {
            let manifest = self.generate_partition_manifest(
                partition_key,
                attempt_usage_by_partition
                    .get(&partition_key)
                    .map(|v| v.len())
                    .unwrap_or(0),
                quota_obs_by_partition
                    .get(&partition_key)
                    .map(|v| v.len())
                    .unwrap_or(0),
                task_outcome_by_partition
                    .get(&partition_key)
                    .map(|v| v.len())
                    .unwrap_or(0),
                review_outcome_by_partition
                    .get(&partition_key)
                    .map(|v| v.len())
                    .unwrap_or(0),
            )?;

            let manifest_dir = self.config.telemetry_repo_path.join("manifests");
            let manifest_path =
                manifest_dir.join(format!("{}.json", partition_key.to_date_string()));

            fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
                .with_context(|| format!("Writing manifest: {}", manifest_path.display()))?;

            log::info!("Generated manifest: {}", manifest_path.display());
        }

        Ok(())
    }

    /// Generate a manifest for a specific partition
    fn generate_partition_manifest(
        &self,
        partition_key: PartitionKey,
        attempt_usage_count: usize,
        quota_obs_count: usize,
        task_outcome_count: usize,
        review_outcome_count: usize,
    ) -> Result<PartitionManifest> {
        let total_records =
            attempt_usage_count + quota_obs_count + task_outcome_count + review_outcome_count;

        // We need to compute SHA-256, but for now we'll leave it empty
        // as we don't have easy access to all the file contents here
        let sha256 = None;

        // Use the partition key to create date strings
        let first_observed_at = format!("{}-01T00:00:00Z", partition_key.to_date_string());
        let last_observed_at = format!("{}-31T23:59:59Z", partition_key.to_date_string());

        Ok(PartitionManifest {
            partition: format!("{}.jsonl", partition_key.to_date_string()),
            records: total_records,
            first_observed_at,
            last_observed_at,
            schema_version: SCHEMA_VERSION,
            sha256,
            record_type_counts: RecordTypeCounts {
                attempt_usage: attempt_usage_count,
                quota_observation: quota_obs_count,
                task_outcome: task_outcome_count,
                review_outcome: review_outcome_count,
            },
        })
    }

    /// Get the number of records exported
    #[allow(dead_code)]
    pub fn records_exported(&self) -> usize {
        self.records_exported
    }

    /// Get the set of exported record IDs
    #[allow(dead_code)]
    pub fn exported_ids(&self) -> &BTreeSet<String> {
        &self.exported_ids
    }
}

/// Export telemetry from ledger entries
#[allow(dead_code)]
pub fn export_telemetry(
    entries: &[LedgerEntry],
    telemetry_repo_path: &Path,
    format: ExportFormat,
    generate_manifests: bool,
    exported_at: Option<&str>,
) -> Result<TelemetryExporter> {
    let config = TelemetryConfig {
        telemetry_repo_path: telemetry_repo_path.to_path_buf(),
        format,
        generate_manifests,
        commit_batch_size: None, // Don't auto-commit for now
    };

    let mut exporter = TelemetryExporter::new(config)?;

    // Load already exported IDs to avoid duplicates
    exporter.load_exported_ids()?;

    // For now, we'll just mark that we've exported these entries
    // In a full implementation, we'd pass the exported_at to the extractor
    let _ = exported_at;

    // Export telemetry records
    exporter.export_from_entries(entries)?;

    Ok(exporter)
}

/// Check if telemetry repository exists and is accessible
pub fn telemetry_repo_exists(telemetry_repo_path: &Path) -> bool {
    telemetry_repo_path.exists() && telemetry_repo_path.is_dir()
}

/// Get the default telemetry repository path
#[allow(dead_code)]
pub fn default_telemetry_repo_path() -> PathBuf {
    // Look for telemetry submodule first
    let repo_root = std::env::current_dir().unwrap_or_default();
    let submodule_path = repo_root.join("telemetry");

    if submodule_path.exists() && submodule_path.is_dir() {
        return submodule_path;
    }

    // Fallback to config directory
    let config_dir = crate::config::default_config_dir();
    config_dir.join("telemetry")
}
