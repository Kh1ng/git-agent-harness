use crate::{
    config::Profile,
    ledger::{FailureClass, FailureStage, LedgerEntry},
    validation_runner::validate_with_exit_code,
    worktree,
};
use anyhow::{bail, Context, Result};
use std::{path::Path, time::Duration};

pub(super) struct JobContract {
    file: String,
    allowed: Option<Vec<String>>,
    commands: Vec<String>,
}

impl JobContract {
    /// `not_handed_over` is true when the work did not come from an operator
    /// passing this file to `gah dispatch`: a provider issue, or a ticket the
    /// loop selected. Their text is never enforced and never executed.
    pub(super) fn load(target: &str, not_handed_over: bool) -> Result<Option<Self>> {
        let path = Path::new(target);
        if not_handed_over
            || target.is_empty()
            || !path.is_file()
            || path.extension().is_none_or(|e| e != "md")
        {
            return Ok(None);
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading job file {target}"))?;
        let allowed = section(&text, "Allowed files", target)?;
        let commands = section(&text, "Verification commands", target)?;
        if let Some(items) = &allowed {
            for item in items {
                if Path::new(item).is_absolute()
                    || item.contains("..")
                    || item.starts_with('\\')
                    || item.as_bytes().get(1) == Some(&b':')
                {
                    bail!("Job file {target}: Allowed files contains invalid repository-relative path: {item}");
                }
            }
        }
        if allowed.is_none() && commands.is_none() {
            return Ok(None);
        }
        Ok(Some(Self {
            file: target.into(),
            allowed,
            commands: commands.unwrap_or_default(),
        }))
    }

    fn scope(&self, profile: &Profile, wt: &Path) -> Result<()> {
        let Some(allowed) = &self.allowed else {
            return Ok(());
        };
        let outside: Vec<_> = worktree::contract_changed_files(wt, &profile.default_target_branch)?
            .into_iter()
            .filter(|path| !allowed.iter().any(|pattern| matches_path(pattern, path)))
            .collect();
        if !outside.is_empty() {
            bail!(
                "Job file {} allows only the listed files: {}\nOut-of-scope paths:\n{}",
                self.file,
                allowed.join(", "),
                outside.join("\n")
            );
        }
        Ok(())
    }
}

/// The contract for this dispatch, if any. Only a direct `gah dispatch` of a
/// local file gets one. The controller sets `dispatch_reason` on every
/// dispatch it starts (`initial`, `retry`, `review`, ...) and the CLI leaves
/// it unset, so a ticket file the loop picked is never enforced: existing
/// tickets already carry a "Verification Commands" section as a hint, and
/// some are written by a planning agent.
pub(super) fn for_dispatch(
    args: &crate::dispatch::DispatchArgs,
    has_issue: bool,
) -> Result<Option<JobContract>> {
    JobContract::load(&args.target, has_issue || args.dispatch_reason.is_some())
}

fn section(text: &str, heading: &str, file: &str) -> Result<Option<Vec<String>>> {
    let mut found = false;
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            if found {
                break;
            }
            found = line
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case(heading);
            continue;
        }
        if !found {
            continue;
        }
        let bullet = ["-", "*", "+"].iter().find_map(|prefix| {
            line.strip_prefix(prefix)
                .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        });
        let Some(bullet) = bullet else {
            continue;
        };
        let bullet = bullet.trim();
        let quoted = bullet
            .split_once('`')
            .and_then(|(_, rest)| rest.split_once('`').map(|(item, _)| item));
        let item = quoted
            .unwrap_or_else(|| {
                bullet
                    .split_once(" (")
                    .map(|(item, _)| item)
                    .unwrap_or(bullet)
            })
            .trim();
        if item.is_empty() {
            bail!("Job file {file}: {heading} contains an empty bullet item");
        }
        items.push(item.into());
    }
    if !found {
        return Ok(None);
    }
    if items.is_empty() {
        bail!("Job file {file}: {heading} must contain a markdown bullet list");
    }
    Ok(Some(items))
}

fn matches_path(pattern: &str, path: &str) -> bool {
    if let Some(dir) = pattern
        .strip_suffix("/**")
        .or_else(|| pattern.strip_suffix('/'))
    {
        return path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'));
    }
    fn segment(pattern: &[u8], value: &[u8]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some((b'*', rest)) => (0..=value.len()).any(|i| segment(rest, &value[i..])),
            Some((head, rest)) => value
                .split_first()
                .is_some_and(|(v, tail)| head == v && segment(rest, tail)),
        }
    }
    let patterns: Vec<_> = pattern.split('/').collect();
    let paths: Vec<_> = path.split('/').collect();
    patterns.len() == paths.len()
        && patterns
            .iter()
            .zip(paths)
            .all(|(p, v)| segment(p.as_bytes(), v.as_bytes()))
}

pub(super) fn validate(
    contract: Option<&JobContract>,
    profile: &Profile,
    wt: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<(), (String, Option<i32>)> {
    if let Some(contract) = contract {
        contract
            .scope(profile, wt)
            .map_err(|error| (error.to_string(), None))?;
        validate_with_exit_code(&contract.commands, wt, env, timeout)?;
    }
    Ok(())
}

pub(super) fn publish_guard(
    contract: Option<&JobContract>,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    wt: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<()> {
    if let Err((error, _)) = validate(contract, profile, wt, env, timeout) {
        ledger.set_failure(
            FailureClass::ValidationFailure,
            FailureStage::PostValidation,
        );
        ledger.validation_result = Some("failed".into());
        bail!("{error}");
    }
    Ok(())
}

pub(super) fn validate_round(
    (contract, profile): (Option<&JobContract>, &Profile),
    wt: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<(), (String, Option<i32>)> {
    validate_with_exit_code(&profile.validation_commands, wt, env, timeout)?;
    validate(contract, profile, wt, env, timeout)
}

#[cfg(test)]
mod tests;
