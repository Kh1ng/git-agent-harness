use super::run_dispatch_and_record;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;

pub(super) fn pending_plan(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> Result<Option<PathBuf>> {
    let entries = crate::ledger::read_entries(cfg)?;
    Ok(entries.into_iter().rev().find_map(|entry| {
        (entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.work_id.as_deref() == Some(work_id)
            && entry.mode == "pm"
            && entry.failure_class.is_none()
            && entry.validation_result.as_deref() != Some("failed"))
        .then(|| entry.session_dir.map(PathBuf::from))
        .flatten()
        .map(|session| session.join("pm-plan-v1.json"))
        .filter(|plan| plan.is_file())
    }))
}

struct PmExecutionContext<'a> {
    cfg: &'a crate::config::GahConfig,
    profile_name: &'a str,
    work_id: &'a str,
    title: Option<&'a str>,
    run_id: &'a str,
}

impl PmExecutionContext<'_> {
    fn append_failure(
        &self,
        error: &anyhow::Error,
        failure_class: crate::ledger::FailureClass,
        failure_stage: crate::ledger::FailureStage,
    ) -> Result<()> {
        let profile = crate::config::get_profile(self.cfg, self.profile_name)?;
        let mut entry = crate::ledger::LedgerEntry::new(
            self.profile_name,
            profile,
            "control-plane",
            "pm_orchestration",
            self.title.unwrap_or(self.work_id),
            Some(self.run_id.to_string()),
            None,
        );
        entry.work_id = Some(self.work_id.to_string());
        entry.source_issue_number = Some(self.work_id.trim_start_matches('#').to_string());
        entry.validation_result = Some("failed".to_string());
        entry.provider_mutation_kind = Some("plan_publish".to_string());
        entry.provider_mutation_status = Some("failed".to_string());
        entry.set_failure(failure_class, failure_stage);
        entry.error_summary = Some(crate::redact::redact(&format!("{error:#}")));
        crate::ledger::append(self.cfg, &entry)?;
        Ok(())
    }
}

pub(super) fn reconcile_parent(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    work_id: &str,
    source_issue_number: &str,
    plan_fingerprint: &str,
    child_issue_numbers: &[String],
) -> Result<String> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let mut entry = crate::ledger::LedgerEntry::new(
        profile_name,
        profile,
        "control-plane",
        "pm_reconcile",
        work_id,
        None,
        None,
    );
    entry.work_id = Some(work_id.to_string());
    entry.source_issue_number = Some(source_issue_number.to_string());
    entry.validation_result = Some("passed".to_string());
    entry.pm_plan_fingerprint = Some(plan_fingerprint.to_string());
    entry.pm_publication_status = Some("completed".to_string());
    entry.pm_child_issue_numbers = child_issue_numbers.to_vec();
    crate::ledger::append(cfg, &entry)?;
    crate::events::record(
        cfg,
        crate::events::EventType::PmReconciled,
        Some(profile_name),
        Some(work_id),
        format!(
            "all {} provider-native child issue(s) are terminal; source remains open",
            child_issue_numbers.len()
        ),
    )?;
    Ok(format!(
        "Reconciled PM parent {work_id}: all {} child issue(s) terminal; source left open",
        child_issue_numbers.len()
    ))
}

pub(super) fn execute(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    ticket_path: &str,
    work_id: &str,
    title: Option<&str>,
    skip_validation_gate: bool,
    route_ready: Option<SyncSender<()>>,
) -> Result<String> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let execution = PmExecutionContext {
        cfg,
        profile_name,
        work_id,
        title,
        run_id: &run_id,
    };
    if let Err(error) = crate::dispatch::validate_pm_source_depth(cfg, profile_name, ticket_path) {
        execution.append_failure(
            &error,
            crate::ledger::FailureClass::HumanBlocked,
            crate::ledger::FailureStage::Preflight,
        )?;
        crate::events::record_with_run_id(
            cfg,
            crate::events::EventType::PmBlocked,
            Some(profile_name),
            Some(work_id),
            Some(&run_id),
            format!("PM source preflight blocked: {error:#}"),
        )?;
        let safe_error = crate::redact::redact(&format!("{error:#}"));
        crate::notifications::notify_terminal_failure(
            cfg,
            profile,
            crate::notifications::TerminalFailurePayload {
                profile: profile_name,
                work_id,
                run_id: &run_id,
                failure_class: crate::ledger::FailureClass::HumanBlocked.as_str(),
                failure_stage: Some(crate::ledger::FailureStage::Preflight.as_str()),
                attempt_count: Some(1),
                error_summary: Some(&safe_error),
                mr_url: None,
            },
        );
        return Err(error).context("validating PM decomposition source limits");
    }
    let mut plan_path = pending_plan(cfg, profile_name, &profile.repo_id, work_id)?;
    if plan_path.is_none() {
        let args = crate::dispatch::DispatchArgs {
            profile: profile_name.to_string(),
            mode: "pm".to_string(),
            backend: "auto".to_string(),
            target: ticket_path.to_string(),
            branch: None,
            mr: None,
            current_branch: false,
            dry_run: false,
            oh_profile: None,
            model: None,
            retries: 0,
            allow_draft_fail: false,
            prod: false,
            issue_intake_override: false,
            allow_unknown_red_baseline: false,
            escalate: false,
            existing_branch: None,
            expected_review_generation: None,
            skip_validation_gate,
            dispatch_reason: Some("pm_decomposition".to_string()),
            work_id: Some(work_id.to_string()),
            run_id: Some(run_id.clone()),
            route_ready,
        };
        match run_dispatch_and_record(cfg, "pm_plan", Some(work_id), &args) {
            Ok(Some(deferred)) => return Ok(deferred),
            Ok(None) => {}
            Err(error) => {
                crate::events::record_with_run_id(
                    cfg,
                    crate::events::EventType::PmFailed,
                    Some(profile_name),
                    Some(work_id),
                    Some(&run_id),
                    format!("PM planning failed: {error:#}"),
                )?;
                return Err(error);
            }
        }
        plan_path = Some(
            PathBuf::from(&profile.artifact_root)
                .join("sessions")
                .join(&run_id)
                .join("pm-plan-v1.json"),
        );
        crate::events::record_with_run_id(
            cfg,
            crate::events::EventType::PmPlanned,
            Some(profile_name),
            Some(work_id),
            Some(&run_id),
            format!("validated PM plan for {ticket_path}"),
        )?;
    }
    let plan_path = plan_path.context("PM planning succeeded without an artifact path")?;
    let summary = match crate::dispatch::publish_pm_plan(cfg, profile_name, &plan_path, false) {
        Ok(summary) => summary,
        Err(error) => {
            execution.append_failure(
                &error,
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::Sync,
            )?;
            let blocked = error.to_string().contains("configured pm_max_")
                || error.to_string().contains("decomposition depth");
            crate::events::record_with_run_id(
                cfg,
                if blocked {
                    crate::events::EventType::PmBlocked
                } else {
                    crate::events::EventType::PmFailed
                },
                Some(profile_name),
                Some(work_id),
                Some(&run_id),
                format!("PM publication failed: {error:#}"),
            )?;
            let safe_error = crate::redact::redact(&format!("{error:#}"));
            crate::notifications::notify_terminal_failure(
                cfg,
                profile,
                crate::notifications::TerminalFailurePayload {
                    profile: profile_name,
                    work_id,
                    run_id: &run_id,
                    failure_class: crate::ledger::FailureClass::HarnessError.as_str(),
                    failure_stage: Some(crate::ledger::FailureStage::Sync.as_str()),
                    attempt_count: Some(1),
                    error_summary: Some(&safe_error),
                    mr_url: None,
                },
            );
            return Err(error).context("publishing bounded PM decomposition");
        }
    };
    crate::events::record_with_run_id(
        cfg,
        if summary.already_published {
            crate::events::EventType::PmDuplicate
        } else {
            crate::events::EventType::PmPublished
        },
        Some(profile_name),
        Some(work_id),
        Some(&run_id),
        format!(
            "PM plan {} {} child issue(s)",
            if summary.already_published {
                "already published"
            } else {
                "published"
            },
            summary.child_issue_numbers.len()
        ),
    )?;
    Ok(format!(
        "Published bounded PM decomposition for {work_id}: {} child issue(s)",
        summary.child_issue_numbers.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::{pending_plan, reconcile_parent};

    #[test]
    fn resumes_exact_successful_session_artifact() {
        let (tmp, mut cfg) = crate::ledger::test_util::test_config();
        let mut profile = crate::ledger::test_util::profile();
        profile.artifact_root = tmp.path().join("artifacts").display().to_string();
        cfg.profiles.insert("real".into(), profile.clone());
        let session = tmp.path().join("artifacts/sessions/run-1");
        std::fs::create_dir_all(&session).unwrap();
        let plan = session.join("pm-plan-v1.json");
        std::fs::write(&plan, "{}").unwrap();
        let mut entry = crate::ledger::LedgerEntry::new(
            "real",
            &profile,
            "claude",
            "pm",
            "#561",
            Some("run-1".into()),
            Some(&session),
        );
        entry.work_id = Some("#561".into());
        entry.validation_result = Some("passed".into());
        crate::ledger::append(&cfg, &entry).unwrap();

        assert_eq!(
            pending_plan(&cfg, "real", &profile.repo_id, "#561").unwrap(),
            Some(plan)
        );
    }

    #[test]
    fn reconciliation_records_completion_without_source_closure() {
        let (_tmp, mut cfg) = crate::ledger::test_util::test_config();
        cfg.profiles
            .insert("real".into(), crate::ledger::test_util::profile());

        let outcome = reconcile_parent(
            &cfg,
            "real",
            "#561",
            "561",
            "fingerprint-a",
            &["600".into(), "601".into()],
        )
        .unwrap();

        assert!(outcome.contains("source left open"));
        let entry = crate::ledger::read_entries(&cfg).unwrap().pop().unwrap();
        assert_eq!(entry.mode, "pm_reconcile");
        assert_eq!(entry.pm_publication_status.as_deref(), Some("completed"));
        assert_eq!(entry.pm_child_issue_numbers, ["600", "601"]);
        assert_eq!(entry.provider_mutation_kind, None);
    }
}
