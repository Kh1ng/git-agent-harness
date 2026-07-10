//! Telemetry Export Module
//!
//! Provides durable, versioned persistence of usage, quota, and outcome telemetry
//! to a separate private dataset repository.
//!
//! This module implements the requirements from TICKET-130 for preserving
//! high-value execution telemetry independently of the operational ledger.

pub mod records;
pub mod extractor;
pub mod exporter;

// Re-export commonly used types and functions
// pub use exporter::{ExportFormat, TelemetryConfig, TelemetryExporter, telemetry_repo_exists};

use crate::config::GahConfig;
use crate::ledger::LedgerEntry;
use anyhow::Result;


/// GroupBy options for telemetry export (matching existing ledger summary)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Backend,
    Model,
}

impl std::str::FromStr for GroupBy {
    type Err = String;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(GroupBy::None),
            "backend" => Ok(GroupBy::Backend),
            "model" => Ok(GroupBy::Model),
            _ => Err(format!("Unknown group by option: {}", s)),
        }
    }
}

/// Main telemetry export function that handles the full workflow
pub fn export_telemetry_from_config(
    cfg: &GahConfig,
    telemetry_repo_path: Option<&str>,
    format: exporter::ExportFormat,
    generate_manifests: bool,
    _group_by: GroupBy,
    since: Option<&str>,
    profile: Option<&str>,
) -> Result<exporter::TelemetryExporter> {
    use crate::ledger::read_entries;
    
    // Load ledger entries
    let entries = read_entries(cfg)?;
    
    // Filter entries based on parameters
    let entries = filter_entries(&entries, profile, since)?;
    
    if entries.is_empty() {
        log::info!("No ledger entries found for telemetry export");
        // Return an exporter with no records exported
        let config = exporter::TelemetryConfig {
            telemetry_repo_path: determine_telemetry_repo_path(telemetry_repo_path),
            format,
            generate_manifests,
            commit_batch_size: None,
        };
        return Ok(exporter::TelemetryExporter::new(config)?);
    }
    
    // Determine telemetry repo path
    let repo_path = determine_telemetry_repo_path(telemetry_repo_path);
    
    // Create and run exporter
    let mut exporter = exporter::TelemetryExporter::new(exporter::TelemetryConfig {
        telemetry_repo_path: repo_path.clone(),
        format,
        generate_manifests,
        commit_batch_size: None,
    })?;
    
    // Load already exported IDs to avoid duplicates
    exporter.load_exported_ids()?;
    
    // Export telemetry records
    let _exported_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc().unix_timestamp().to_string());
    
    exporter.export_from_entries(&entries)?;
    
    Ok(exporter)
}

/// Filter entries based on profile and time
fn filter_entries(
    entries: &[LedgerEntry],
    profile: Option<&str>,
    since: Option<&str>,
) -> Result<Vec<LedgerEntry>> {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    
    let mut filtered = entries.to_vec();
    
    // Filter by profile if specified
    if let Some(profile_name) = profile {
        filtered.retain(|e| e.profile == profile_name);
    }
    
    // Filter by time if specified
    if let Some(since_str) = since {
        let since_time: OffsetDateTime = if let Ok(datetime) = OffsetDateTime::parse(since_str, &Rfc3339) {
            // Absolute datetime
            datetime
        } else {
            // Try parsing as date only
            let since_date = since_str.trim();
            let date_parts: Vec<&str> = since_date.split('-').collect();
            if date_parts.len() == 3 {
                let year = date_parts[0].parse::<i32>().unwrap_or(2024);
                let month = date_parts[1].parse::<u8>().unwrap_or(1);
                let day = date_parts[2].parse::<u8>().unwrap_or(1);
                
                // Create a time at midnight UTC
                let month_enum = time::Month::try_from(month).unwrap_or(time::Month::January);
                let date = time::Date::from_calendar_date(year, month_enum, day)?;
                let primitive_datetime = date.with_hms_milli(0, 0, 0, 0)?;
                // Convert PrimitiveDateTime to OffsetDateTime (UTC)
                OffsetDateTime::from(primitive_datetime.assume_utc())
            } else {
                // Default to all entries if we can't parse
                return Ok(filtered);
            }
        };
        
        filtered.retain(|entry| {
            if let Ok(entry_time) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
                entry_time >= since_time
            } else {
                // If we can't parse the timestamp, include it
                true
            }
        });
    }
    
    Ok(filtered)
}

/// Determine the telemetry repository path
fn determine_telemetry_repo_path(telemetry_repo_path: Option<&str>) -> std::path::PathBuf {
    if let Some(path) = telemetry_repo_path {
        return std::path::PathBuf::from(path);
    }
    
    // Look for telemetry submodule in current directory
    let current_dir = std::env::current_dir().unwrap_or_default();
    let submodule_path = current_dir.join("telemetry");
    
    if submodule_path.exists() && submodule_path.is_dir() {
        return submodule_path;
    }
    
    // Fallback to config directory
    let config_dir = crate::config::default_config_dir();
    config_dir.join("telemetry")
}

/// Tests for telemetry functionality
#[cfg(test)]
pub mod tests;

/// CLI interface for telemetry commands
pub mod cli {
    use super::*;
    use anyhow::Result;
    use std::path::PathBuf;
    
    /// Run telemetry export command
    pub fn run_export(
        telemetry_repo_path: Option<&str>,
        format: Option<exporter::ExportFormat>,
        output: Option<&str>,
        since: Option<&str>,
        profile: Option<&str>,
        group_by: Option<GroupBy>,
        generate_manifests: bool,
        config_path: Option<&str>,
    ) -> Result<()> {
        let cfg = crate::config::load(config_path)?;
        
        // Determine telemetry repo path
        let repo_path = if let Some(path) = telemetry_repo_path {
            PathBuf::from(path)
        } else {
            determine_telemetry_repo_path(None)
        };
        
        // Determine output path
        let output_path = if let Some(output) = output {
            PathBuf::from(output)
        } else {
            repo_path.clone()
        };
        
        // Create output directory if it doesn't exist
        if !output_path.exists() {
            std::fs::create_dir_all(&output_path)?;
        }
        
        let export_format = format.unwrap_or(exporter::ExportFormat::Jsonl);
        let group_by_option = group_by.unwrap_or(GroupBy::None);
        
        log::info!("Exporting telemetry to: {}", output_path.display());
        log::info!("Format: {:?}", export_format);
        log::info!("Generate manifests: {}", generate_manifests);
        
        let _exporter = export_telemetry_from_config(
            &cfg,
            Some(output_path.to_str().unwrap()),
            export_format,
            generate_manifests,
            group_by_option,
            since,
            profile,
        )?;
        
        log::info!("Telemetry export completed successfully");
        Ok(())
    }
    
    /// Run telemetry status command
    pub fn run_status(
        telemetry_repo_path: Option<&str>,
        config_path: Option<&str>,
    ) -> Result<()> {
        let _cfg = crate::config::load(config_path)?;
        
        // Determine telemetry repo path
        let repo_path = if let Some(path) = telemetry_repo_path {
            PathBuf::from(path)
        } else {
            determine_telemetry_repo_path(None)
        };
        
        if !exporter::telemetry_repo_exists(&repo_path) {
            println!("Telemetry repository not found at: {}", repo_path.display());
            println!("Telemetry export is optional and not required for normal operation.");
            return Ok(());
        }
        
        // Count existing records
        let mut total_records = 0;
        let mut records_by_type: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        
        let raw_dir = repo_path.join("raw");
        if raw_dir.exists() {
            for subdir in ["usage", "quota", "outcomes"] {
                let subdir_path = raw_dir.join(subdir);
                if subdir_path.exists() {
                    count_records_in_directory(&subdir_path, &mut total_records, &mut records_by_type)?;
                }
            }
        }
        
        println!("Telemetry Repository Status");
        println!("==========================");
        println!("Path: {}", repo_path.display());
        println!("Exists: {}", exporter::telemetry_repo_exists(&repo_path));
        println!("Total Records: {}", total_records);
        println!();
        println!("Records by type:");
        for (record_type, count) in &records_by_type {
            println!("  {}: {}", record_type, count);
        }
        
        // Check manifests
        let manifests_dir = repo_path.join("manifests");
        if manifests_dir.exists() {
            let manifest_count = std::fs::read_dir(&manifests_dir)?.count();
            println!();
            println!("Manifests: {}", manifest_count);
        }
        
        Ok(())
    }
    
    /// Count records in a directory
    fn count_records_in_directory(
        dir: &std::path::Path,
        total_records: &mut usize,
        records_by_type: &mut std::collections::BTreeMap<String, usize>,
    ) -> Result<()> {
        if !dir.exists() || !dir.is_dir() {
            return Ok(());
        }
        
        let mut dirs_to_visit = vec![dir.to_path_buf()];
        
        while let Some(current_dir) = dirs_to_visit.pop() {
            for entry in std::fs::read_dir(&current_dir)? {
                let entry = entry?;
                let path = entry.path();
                
                if path.is_dir() {
                    dirs_to_visit.push(path);
                } else if path.extension().map(|e| e.to_string_lossy() == "jsonl").unwrap_or(false) {
                    // Count records in JSONL file
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let record_count = content.lines()
                            .filter(|line| !line.trim().is_empty())
                            .filter(|line| {
                                // Try to parse as JSON to verify it's valid
                                serde_json::from_str::<serde_json::Value>(line).is_ok()
                            })
                            .count();
                        
                        *total_records += record_count;
                        
                        // Determine record type from file path
                        let record_type = if path.parent().map(|p| p.file_name().map(|n| n.to_string_lossy() == "usage").unwrap_or(false)).unwrap_or(false) {
                            "attempt_usage".to_string()
                        } else if path.parent().map(|p| p.file_name().map(|n| n.to_string_lossy() == "quota").unwrap_or(false)).unwrap_or(false) {
                            "quota_observation".to_string()
                        } else if path.parent().map(|p| p.file_name().map(|n| n.to_string_lossy() == "outcomes").unwrap_or(false)).unwrap_or(false) {
                            "outcome".to_string()
                        } else {
                            "unknown".to_string()
                        };
                        
                        *records_by_type.entry(record_type).or_insert(0) += record_count;
                    }
                }
            }
        }
        
        Ok(())
    }
}