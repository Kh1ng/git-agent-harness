use super::NextAction;
use anyhow::Result;

pub(super) fn execute(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    action: &NextAction,
) -> Result<String> {
    let NextAction::MergeMr {
        branch,
        work_id,
        mr_url,
        review_generation,
        ..
    } = action
    else {
        unreachable!("merge executor received a non-merge action")
    };
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let merge_policy = profile
        .effective_routing(&cfg.defaults)
        .merge_policy
        .unwrap_or_default();
    let run_id = uuid::Uuid::new_v4().to_string();
    crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::DispatchStarted,
        Some(profile_name),
        action.work_id(),
        Some(&run_id),
        "merge",
    )?;
    let gitlab_mwps =
        merge_policy == crate::config::MergePolicy::GitlabMwps && profile.provider == "gitlab";
    let expected_generation = review_generation.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "merge for branch '{branch}' has no current review generation; re-run review"
        )
    })?;
    let result = if gitlab_mwps {
        // GitLab enforces the CI gate natively after MWPS is set. The exact
        // reviewed generation and source SHA are still checked before arming
        // it, just as they are for an immediate provider merge.
        crate::provider::gitlab_set_mwps(profile, branch, expected_generation)
    } else {
        crate::dispatch::merge_branch(
            cfg,
            profile,
            branch,
            work_id,
            mr_url,
            Some(expected_generation),
            Some(&run_id),
        )
    };
    let outcome = match &result {
        Ok(()) if gitlab_mwps => {
            format!("Set GitLab merge-when-pipeline-succeeds on branch '{branch}'")
        }
        Ok(()) => format!("Merged MR on branch '{branch}'"),
        Err(error) => format!("Merge failed for branch '{branch}': {error:#}"),
    };
    crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::DispatchFinished,
        Some(profile_name),
        action.work_id(),
        Some(&run_id),
        format!("merge: {outcome}"),
    )?;
    Ok(outcome)
}
