// Command execution for `gah ledger` (ticket #409).

use anyhow::Result;

use crate::cli::args::LedgerCommands;
use crate::{config, ledger};

use serde_json;

pub fn run(command: LedgerCommands) -> Result<()> {
    match command {
        LedgerCommands::RepairTail {
            config_path,
            dry_run,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let repaired = ledger::repair_truncated_tail(&cfg, dry_run)?;
            match repaired.backup_path {
                Some(path) if dry_run => println!(
                    "Dry run: would back up and remove {} truncated bytes; backup path: {}",
                    repaired.dropped_bytes,
                    path.display()
                ),
                Some(path) => println!(
                    "Repaired ledger tail: backed up and removed {} truncated bytes at {}",
                    repaired.dropped_bytes,
                    path.display()
                ),
                None => println!("Ledger tail is complete; no repair needed."),
            }
        }
        LedgerCommands::Summary {
            since,
            profile,
            config_path,
            json,
            group_by,
        } => ledger::summary::run_with_json(
            &since,
            profile.as_deref(),
            config_path.as_deref(),
            json,
            group_by,
        )?,
        LedgerCommands::Reconcile {
            profile,
            config_path,
            json,
            dry_run,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            ledger::reconcile::run(&cfg, &profile, json, dry_run)?;
        }
        LedgerCommands::Work {
            work_id,
            config_path,
            json,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let mut entries = ledger::entries_for_work_id(&cfg, &work_id)?;
            entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            if json {
                println!("{}", serde_json::to_string(&entries)?);
            } else if entries.is_empty() {
                println!("No ledger entries found for work item '{}'.", work_id);
            } else {
                println!("Work item: {} ({} entries)", work_id, entries.len());
                for e in &entries {
                    let cost = e
                        .usage
                        .actual_cost_usd
                        .or(e.usage.estimated_cost_usd)
                        .map(|c| format!("${c:.4}"))
                        .unwrap_or_else(|| "unknown cost".into());
                    println!(
                        "  {}  {}  {}/{}  validation={} failure={} duration={} {}",
                        e.timestamp,
                        e.mode,
                        e.effective_backend,
                        e.effective_model.as_deref().unwrap_or("?"),
                        e.validation_result.as_deref().unwrap_or("-"),
                        e.failure_class.as_deref().unwrap_or("-"),
                        e.duration_seconds
                            .map(|d| format!("{d:.0}s"))
                            .unwrap_or_else(|| "unknown".into()),
                        cost,
                    );
                }
            }
        }
        LedgerCommands::ClearAttempts {
            profile,
            work_id,
            config_path,
            dry_run,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_clear_attempts(&profile, prof, &work_id);
            if dry_run {
                println!(
                    "Dry run: would append tombstone entry for work_id '{}':",
                    work_id
                );
                println!("{}", serde_json::to_string_pretty(&entry)?);
            } else {
                let path = ledger::append(&cfg, &entry)?;
                println!(
                    "Appended tombstone entry for work_id '{}' to {}",
                    work_id,
                    path.display()
                );
            }
        }
    }
    Ok(())
}
