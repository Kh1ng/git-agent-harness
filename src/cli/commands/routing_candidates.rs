//! Issue #149: ordered routing-candidate editing for a profile. The lists
//! are pm / improve / review / escalatory. Every mutation resolves the
//! EFFECTIVE list (profile -> repo defaults), applies the change, writes the
//! full list back into the profile section (candidate lists replace
//! wholesale — nothing inherited is lost), validates, then saves. The
//! printed order is the pre-save preview.

use anyhow::{bail, Result};

use crate::cli::args::RoutingCandidateCommands;
use crate::config::{self, CandidateConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateList {
    Pm,
    Improve,
    Review,
    Escalatory,
}

impl CandidateList {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pm" => Some(Self::Pm),
            "improve" => Some(Self::Improve),
            "review" => Some(Self::Review),
            "escalatory" => Some(Self::Escalatory),
            _ => None,
        }
    }

    fn get(self, policy: &crate::config::RoutingPolicy) -> Option<Vec<CandidateConfig>> {
        match self {
            Self::Pm => policy.pm_candidates.clone(),
            Self::Improve => policy.improve_candidates.clone(),
            Self::Review => policy.review_candidates.clone(),
            Self::Escalatory => (!policy.escalatory_reviewers.is_empty())
                .then(|| policy.escalatory_reviewers.clone()),
        }
    }

    fn set(self, policy: &mut crate::config::RoutingPolicy, list: Vec<CandidateConfig>) {
        match self {
            Self::Pm => policy.pm_candidates = Some(list),
            Self::Improve => policy.improve_candidates = Some(list),
            Self::Review => policy.review_candidates = Some(list),
            Self::Escalatory => policy.escalatory_reviewers = list,
        }
    }
}

pub(crate) fn parse_list(raw: &str) -> Result<CandidateList> {
    CandidateList::parse(raw).ok_or_else(|| {
        anyhow::anyhow!("unrecognized list '{raw}' (expected pm|improve|review|escalatory)")
    })
}

fn print_order(list: &[CandidateConfig], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &list
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| {
                        serde_json::json!({
                            "index": index,
                            "backend": candidate.backend,
                            "instance": candidate.instance,
                            "model": candidate.model,
                            "priority": candidate.priority,
                            "included_in_quota": candidate.included_in_quota,
                            "requires_approval": candidate.requires_approval,
                        })
                    })
                    .collect::<Vec<_>>()
            )
            .expect("candidate projection serializes")
        );
        return;
    }
    for (index, candidate) in list.iter().enumerate() {
        println!(
            "{index}: {}{}{}{}",
            candidate.backend,
            candidate
                .instance
                .as_deref()
                .map(|instance| format!("[{instance}]"))
                .unwrap_or_default(),
            candidate
                .model
                .as_deref()
                .map(|model| format!("/{}", model))
                .unwrap_or_default(),
            if candidate.requires_approval {
                " (approval required)"
            } else {
                ""
            },
        );
    }
}

/// Shared mutation core: resolve the effective list, apply `change`, write it
/// into the profile section, validate, save (unless dry-run), print the
/// resulting order.
fn apply(
    cfg: &mut config::GahConfig,
    profile_name: &str,
    list: CandidateList,
    change: impl FnOnce(&mut Vec<CandidateConfig>) -> Result<()>,
    config_path: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let effective = {
        let profile = config::get_profile(cfg, profile_name)?;
        let routing = profile.effective_routing(&cfg.defaults);
        list.get(&routing).unwrap_or_default()
    };
    let mut updated = effective.clone();
    change(&mut updated)?;

    let defaults = cfg.defaults.clone();
    {
        let profile_config = config::get_profile_mut(cfg, profile_name)?;
        list.set(&mut profile_config.routing, updated.clone());
        // Validate before saving: candidate instance references and backend
        // contracts must hold with the new order in place.
        if let Err(errors) = config::check_profile_backend_instances(&defaults, profile_config) {
            bail!(
                "routing candidates invalid for profile '{}': {}",
                profile_name,
                errors.join("; ")
            );
        }
    }

    if !dry_run {
        config::save(cfg, config_path)?;
    }
    print_order(&updated, json);
    if dry_run && !json {
        println!("[dry-run] not saved");
    }
    Ok(())
}

pub(crate) fn run(command: RoutingCandidateCommands) -> Result<()> {
    match command {
        RoutingCandidateCommands::Add {
            profile,
            list,
            backend,
            instance,
            model,
            quota_pool,
            priority,
            included_in_quota,
            marginal_cost_usd,
            requires_approval,
            config_path,
            dry_run,
            json,
        } => {
            let list = parse_list(&list)?;
            let mut cfg = config::load(config_path.as_deref())?;
            if config::get_profile(&cfg, &profile).is_err() {
                bail!("profile '{profile}' is not configured");
            }
            let candidate = CandidateConfig {
                backend: backend.clone(),
                instance: instance.filter(|value| !value.trim().is_empty()),
                model: model.filter(|value| !value.trim().is_empty()),
                quota_pool: quota_pool.filter(|value| !value.trim().is_empty()),
                priority,
                included_in_quota,
                marginal_cost_usd,
                quota_usage_percent: None,
                quota_days_remaining: None,
                requires_approval,
            };
            apply(
                &mut cfg,
                &profile,
                list,
                |candidates: &mut Vec<CandidateConfig>| {
                    candidates.push(candidate);
                    Ok(())
                },
                config_path.as_deref(),
                dry_run,
                json,
            )
        }
        RoutingCandidateCommands::Remove {
            profile,
            list,
            index,
            config_path,
            dry_run,
            json,
        } => {
            let list = parse_list(&list)?;
            let mut cfg = config::load(config_path.as_deref())?;
            if config::get_profile(&cfg, &profile).is_err() {
                bail!("profile '{profile}' is not configured");
            }
            apply(
                &mut cfg,
                &profile,
                list,
                |candidates: &mut Vec<CandidateConfig>| {
                    if index >= candidates.len() {
                        bail!(
                            "index {index} is out of range (list has {} entries)",
                            candidates.len()
                        );
                    }
                    candidates.remove(index);
                    Ok(())
                },
                config_path.as_deref(),
                dry_run,
                json,
            )
        }
        RoutingCandidateCommands::Move {
            profile,
            list,
            from,
            to,
            config_path,
            dry_run,
            json,
        } => {
            let list = parse_list(&list)?;
            let mut cfg = config::load(config_path.as_deref())?;
            if config::get_profile(&cfg, &profile).is_err() {
                bail!("profile '{profile}' is not configured");
            }
            apply(
                &mut cfg,
                &profile,
                list,
                |candidates: &mut Vec<CandidateConfig>| {
                    if from >= candidates.len() || to >= candidates.len() {
                        bail!(
                            "from/to out of range (list has {} entries)",
                            candidates.len()
                        );
                    }
                    let candidate = candidates.remove(from);
                    candidates.insert(to, candidate);
                    Ok(())
                },
                config_path.as_deref(),
                dry_run,
                json,
            )
        }
    }
}
