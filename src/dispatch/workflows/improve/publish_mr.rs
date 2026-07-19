use crate::config::{GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::notifications::{notify_event, NotifyEvent};
use crate::provider;
use anyhow::{Context, Result};

#[allow(clippy::too_many_arguments)]
pub(super) fn publish_or_update_mr(
    cfg: &GahConfig,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    branch: &str,
    mr_title: &str,
    mr_body: &str,
    is_manual_fix: bool,
    effective_backend: &str,
    effective_model: Option<&str>,
) -> Result<()> {
    if is_manual_fix {
        let existing = provider::find_review_target_by_branch(profile, branch)
            .with_context(|| format!("resolving existing PR/MR for repaired branch '{branch}'"))?;
        ledger.mr_url = Some(existing.url.clone());
        provider::set_review_state_labels(profile, branch, &["gah-review-escalating"])
            .context("transitioning repaired PR/MR back to review")?;
        println!("Updated existing MR: {}", existing.url);
    } else {
        ledger.mr_attempted = true;
        let mr = match provider::create_draft_mr(profile, branch, mr_title, mr_body) {
            Ok(mr) => mr,
            Err(err) => {
                if profile.provider == "gitlab" {
                    ledger.set_failure(
                        crate::ledger::FailureClass::EnvironmentError,
                        crate::ledger::FailureStage::MrCreate,
                    );
                }
                return Err(err);
            }
        };
        ledger.mr_created = true;
        ledger.mr_url = Some(mr.url.clone());
        println!("Draft MR: {}", mr.url);
        notify_event(
            cfg,
            profile,
            NotifyEvent::MrCreated {
                url: &mr.url,
                work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
                backend: effective_backend,
                model: effective_model.unwrap_or("unknown"),
            },
        );
    }
    Ok(())
}
