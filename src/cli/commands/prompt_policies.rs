use anyhow::{Context, Result};

use crate::cli::args::PromptPolicyCommands;
use crate::prompt_policy::{self, PromptPolicySlot, PromptPolicyTarget};

pub(crate) fn run(command: PromptPolicyCommands) -> Result<()> {
    match command {
        PromptPolicyCommands::Show {
            profile,
            config_path,
            json,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let profile_config = crate::config::get_profile(&cfg, &profile)?;
            let summary = prompt_policy::summary(&profile, profile_config)?;
            print_summary(&summary, json)?;
        }
        PromptPolicyCommands::Set {
            profile,
            slot,
            task_class,
            reviewer_tier,
            content,
            expected_revision,
            config_path,
            dry_run,
            json,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let profile_config = crate::config::get_profile(&cfg, &profile)?;
            let slot = parse_slot(&slot)?;
            let result = prompt_policy::set(
                &profile,
                profile_config,
                PromptPolicyTarget {
                    slot,
                    task_class: task_class.as_deref(),
                    reviewer_tier: reviewer_tier.as_deref(),
                },
                &content,
                expected_revision,
                dry_run,
            )?;
            record_mutation(&cfg, &profile, "set", &result)?;
            print_result(&result, json)?;
        }
        PromptPolicyCommands::Reset {
            profile,
            slot,
            task_class,
            reviewer_tier,
            expected_revision,
            config_path,
            dry_run,
            json,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let profile_config = crate::config::get_profile(&cfg, &profile)?;
            let slot = parse_slot(&slot)?;
            let result = prompt_policy::reset(
                &profile,
                profile_config,
                PromptPolicyTarget {
                    slot,
                    task_class: task_class.as_deref(),
                    reviewer_tier: reviewer_tier.as_deref(),
                },
                expected_revision,
                dry_run,
            )?;
            record_mutation(&cfg, &profile, "reset", &result)?;
            print_result(&result, json)?;
        }
        PromptPolicyCommands::Rollback {
            profile,
            to_revision,
            expected_revision,
            config_path,
            dry_run,
            json,
        } => {
            let cfg = crate::config::load(config_path.as_deref())?;
            let profile_config = crate::config::get_profile(&cfg, &profile)?;
            let result = prompt_policy::rollback(
                &profile,
                profile_config,
                to_revision,
                expected_revision,
                dry_run,
            )?;
            record_mutation(&cfg, &profile, "rollback", &result)?;
            print_result(&result, json)?;
        }
    }
    Ok(())
}

fn parse_slot(value: &str) -> Result<PromptPolicySlot> {
    PromptPolicySlot::parse(value).with_context(|| {
        format!(
            "unrecognized prompt policy slot '{value}' (expected worker_guidance|reviewer_guidance)"
        )
    })
}

fn record_mutation(
    cfg: &crate::config::GahConfig,
    profile: &str,
    action: &str,
    result: &prompt_policy::PromptPolicyMutationResult,
) -> Result<()> {
    if !result.changed || result.dry_run {
        return Ok(());
    }
    crate::events::record(
        cfg,
        crate::events::EventType::PromptPolicyChanged,
        Some(profile),
        None,
        serde_json::json!({
            "action": action,
            "previous_revision": result.previous_revision,
            "revision": result.revision,
        })
        .to_string(),
    )
}

fn print_summary(summary: &prompt_policy::PromptPolicySummary, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(summary)?);
        return Ok(());
    }
    println!("Prompt policy revision: {}", summary.revision);
    for policy in &summary.policies {
        println!(
            "{} task={} tier={} source={} version={} bytes={} hash={}",
            policy.slot,
            policy.task_class.as_deref().unwrap_or("any"),
            policy.reviewer_tier.as_deref().unwrap_or("any"),
            policy.source,
            policy.version,
            policy.byte_size,
            policy.sha256,
        );
    }
    Ok(())
}

fn print_result(result: &prompt_policy::PromptPolicyMutationResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("{}", result.preview_diff);
        println!(
            "Prompt policy revision: {}{}",
            result.revision,
            if result.dry_run { " (dry run)" } else { "" }
        );
    }
    Ok(())
}
