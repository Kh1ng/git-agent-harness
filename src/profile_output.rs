use crate::config::{GahConfig, WakeAutonomy};
use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
struct ProfileSummary<'a> {
    name: &'a str,
    display_name: &'a str,
    provider: &'a str,
    repo: &'a str,
    local_path: &'a str,
    web_url: Option<String>,
    max_parallel_workers: Option<u32>,
    max_open_managed_mrs: u32,
    validation_timeout_seconds: u64,
    manager_wake_autonomy: &'a str,
}

pub(crate) fn list_json(cfg: &GahConfig) -> Result<String> {
    let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
    names.sort_unstable();
    let summaries = names
        .into_iter()
        .map(|name| {
            let profile = &cfg.profiles[name];
            ProfileSummary {
                name,
                display_name: &profile.display_name,
                provider: &profile.provider,
                repo: &profile.repo,
                local_path: &profile.local_path,
                web_url: profile.web_url(),
                max_parallel_workers: profile.max_parallel_workers,
                max_open_managed_mrs: profile.max_open_managed_mrs(),
                validation_timeout_seconds: profile.validation_timeout_seconds(),
                manager_wake_autonomy: match profile.manager_wake_autonomy {
                    WakeAutonomy::Off => "off",
                    WakeAutonomy::ReviewOnly => "review_only",
                    WakeAutonomy::Full => "full",
                },
            }
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string(&summaries)?)
}
