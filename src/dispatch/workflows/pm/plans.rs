//! Profile-scoped PM artifacts for remote clients. IDs name dispatch sessions,
//! never paths; the publisher remains the only owner of provider mutations.
use super::publish::{
    load_or_initialize_state, parse_issue_number, plan_fingerprint, publication_state_path,
    read_artifact, PublicationState,
};
use super::PmPlanArtifact;
use crate::config::{self, GahConfig, Profile};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub(crate) struct PlanDetail {
    pub schema_version: u32,
    pub generated_at: String,
    pub profile: String,
    pub id: String,
    pub provider: String,
    pub repo: String,
    pub source_work_id: String,
    pub updated_at: String,
    pub artifact: PmPlanArtifact,
    pub publication: PublicationState,
    pub failures: Vec<PlanFailure>,
}

#[derive(Serialize)]
pub(crate) struct PlanFailure {
    pub timestamp: String,
    pub message: String,
}

#[derive(Serialize)]
pub(crate) struct PlanSummary {
    id: String,
    title: String,
    source_work_id: String,
    ticket_count: usize,
    updated_at: String,
    publication_status: String,
    plan_fingerprint: String,
}

#[derive(Serialize)]
pub(crate) struct PlanList {
    schema_version: u32,
    generated_at: String,
    profile: String,
    plans: Vec<PlanSummary>,
    next_cursor: Option<String>,
    errors: Vec<PlanReadError>,
}

#[derive(Serialize)]
struct PlanReadError {
    plan_id: String,
    message: String,
}

#[derive(Serialize)]
pub(crate) struct PlanOperation {
    pub schema_version: u32,
    pub generated_at: String,
    pub dry_run: bool,
    pub success: bool,
    pub plan: PlanDetail,
    pub output: Vec<String>,
    pub error: Option<String>,
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id != "."
            && id != ".."
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')),
        "plan ID must be a single session identifier, not a path"
    );
    Ok(())
}

fn sessions_root(profile: &Profile) -> Result<PathBuf> {
    let directory = Path::new(&profile.artifact_root).join("sessions");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_dir(),
            "sessions root must be a regular directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(directory),
        Err(error) => return Err(error.into()),
    }
    let root = fs::canonicalize(&profile.artifact_root)?;
    let directory = fs::canonicalize(directory)?;
    ensure!(
        directory.parent() == Some(root.as_path()),
        "sessions root is outside this profile"
    );
    Ok(directory)
}

/// Resolve only regular files in a direct, non-symlinked session directory.
/// Configuration selects the root; neither request paths nor plan contents can move it.
pub(crate) fn plan_path(cfg: &GahConfig, profile_name: &str, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    let profile = config::get_profile(cfg, profile_name)?;
    let root = sessions_root(profile)?;
    let session = root.join(id);
    ensure!(
        fs::symlink_metadata(&session)?.file_type().is_dir(),
        "plan session must be a regular directory"
    );
    ensure!(
        fs::canonicalize(&session)?.parent() == Some(root.as_path()),
        "plan session is outside this profile"
    );
    let path = session.join("pm-plan-v1.json");
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_file(),
        "plan artifact must be a regular file"
    );
    let state_path = publication_state_path(&path)?;
    match fs::symlink_metadata(&state_path) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_file() && metadata.len() <= 1_000_000,
            "publication state must be a bounded regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn read_plan(cfg: &GahConfig, profile_name: &str, id: &str) -> Result<PlanDetail> {
    let path = plan_path(cfg, profile_name, id)?;
    let profile = config::get_profile(cfg, profile_name)?;
    let artifact = read_artifact(&path, profile)?;
    let fingerprint = plan_fingerprint(&artifact)?;
    let source = parse_issue_number(&artifact.target)?;
    let state_path = publication_state_path(&path)?;
    let publication =
        load_or_initialize_state(&state_path, profile_name, profile, &source, &fingerprint)?;
    let modified = fs::metadata(&path)?.modified()?;
    Ok(PlanDetail {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        profile: profile_name.to_string(),
        id: id.to_string(),
        provider: profile.provider.clone(),
        repo: profile.repo.clone(),
        source_work_id: format!("#{source}"),
        updated_at: chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339(),
        artifact,
        publication,
        failures: Vec::new(),
    })
}

pub(crate) fn show(cfg: &GahConfig, profile_name: &str, id: &str) -> Result<PlanDetail> {
    let mut detail = read_plan(cfg, profile_name, id)?;
    let profile = config::get_profile(cfg, profile_name)?;
    let fingerprint = &detail.publication.plan_fingerprint;
    let mut failures = crate::ledger::read_entries(cfg)?
        .into_iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.repo_id == profile.repo_id
                && (entry.session_id.as_deref() == Some(id)
                    || entry.pm_plan_fingerprint.as_deref() == Some(fingerprint.as_str()))
                && entry.error_summary.is_some()
        })
        .map(|entry| PlanFailure {
            timestamp: entry.timestamp,
            message: crate::redact::redact(entry.error_summary.as_deref().unwrap_or_default()),
        })
        .collect::<Vec<_>>();
    failures.reverse();
    failures.truncate(20);
    detail.failures = failures;
    Ok(detail)
}

/// Cursor order is the stable session ID order, not an inferred plan creation time.
/// Unreadable artifacts stay visible as errors rather than becoming healthy empty data.
pub(crate) fn list(
    cfg: &GahConfig,
    profile_name: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<PlanList> {
    ensure!(
        (1..=100).contains(&limit),
        "limit must be between 1 and 100"
    );
    if let Some(cursor) = cursor {
        validate_id(cursor)?;
    }
    let profile = config::get_profile(cfg, profile_name)?;
    let directory = sessions_root(profile)?;
    let mut ids = Vec::new();
    match fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if cursor.is_some_and(|cursor| id.as_str() <= cursor) || validate_id(&id).is_err() {
                    continue;
                }
                // Symlink sessions are not inspected. They may lead to another profile.
                if entry.file_type()?.is_dir()
                    && fs::symlink_metadata(entry.path().join("pm-plan-v1.json")).is_ok()
                {
                    ids.push(id);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    ids.sort();
    let next_cursor = (ids.len() > limit).then(|| ids[limit - 1].clone());
    ids.truncate(limit);
    let mut plans = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        match read_plan(cfg, profile_name, &id) {
            Ok(detail) => plans.push(PlanSummary {
                id,
                title: detail.artifact.plan.title,
                source_work_id: detail.source_work_id,
                ticket_count: detail.artifact.ticket_count,
                updated_at: detail.updated_at,
                publication_status: detail.publication.status,
                plan_fingerprint: detail.publication.plan_fingerprint,
            }),
            Err(error) => errors.push(PlanReadError {
                plan_id: id,
                message: crate::redact::redact(&error.to_string()),
            }),
        }
    }
    Ok(PlanList {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        profile: profile_name.to_string(),
        plans,
        next_cursor,
        errors,
    })
}

/// Call while holding the profile lock. Approval is bound to the reviewed artifact;
/// publication still enforces existing backend policy, idempotency, and resume rules.
pub(crate) fn publish(
    cfg: &GahConfig,
    profile_name: &str,
    id: &str,
    expected_fingerprint: Option<&str>,
    dry_run: bool,
) -> Result<PlanOperation> {
    let before = show(cfg, profile_name, id)?;
    if !dry_run {
        ensure!(
            expected_fingerprint == Some(before.publication.plan_fingerprint.as_str()),
            "publication requires the reviewed plan fingerprint"
        );
    }
    let path = plan_path(cfg, profile_name, id)?;
    let result =
        crate::dispatch::publish_pm_plan(cfg, profile_name, &path, dry_run, expected_fingerprint);
    let (success, output, error) = match result {
        Ok(summary) => (true, summary.output, None),
        Err(error) => {
            let message = crate::redact::redact(&format!("{error:#}"));
            if !dry_run {
                let profile = config::get_profile(cfg, profile_name)?;
                let mut entry = crate::ledger::LedgerEntry::new(
                    profile_name,
                    profile,
                    "control-plane",
                    "pm_publish",
                    &before.source_work_id,
                    None,
                    path.parent(),
                );
                entry.work_id = Some(before.source_work_id.clone());
                entry.pm_plan_fingerprint = Some(before.publication.plan_fingerprint.clone());
                entry.set_failure(
                    crate::ledger::FailureClass::HarnessError,
                    crate::ledger::FailureStage::Sync,
                );
                entry.error_summary = Some(message.clone());
                crate::ledger::append(cfg, &entry)?;
            }
            (false, Vec::new(), Some(message))
        }
    };
    Ok(PlanOperation {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        dry_run,
        success,
        plan: show(cfg, profile_name, id)?,
        output,
        error,
    })
}

#[cfg(test)]
mod tests;
