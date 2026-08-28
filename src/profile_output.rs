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
    /// Stable repo identity used for GAH-owned branch and worktree naming
    /// (e.g. `gah/<repo_id>-<ts>`, `gah-chat-<repo_id>-<session>`). Clients
    /// that materialize worktrees MUST use this rather than deriving from
    /// `repo`, or prune's prefix matching silently misses their worktrees.
    repo_id: &'a str,
    /// Effective worktree root (defaults.worktree_base). Chat sessions (and
    /// any other client that materializes worktrees) create theirs here so
    /// `gah prune` sees them under the same base with the same naming
    /// conventions -- chat worktrees use the `gah-chat-<repo_id>-` prefix.
    worktree_base: &'a str,
    web_url: Option<String>,
    max_parallel_workers: Option<u32>,
    max_open_managed_mrs: u32,
    validation_timeout_seconds: u64,
    manager_wake_autonomy: &'a str,
    delivery_mode: &'a str,
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
                repo_id: &profile.repo_id,
                worktree_base: &cfg.defaults.worktree_base,
                web_url: profile.web_url(),
                max_parallel_workers: profile.max_parallel_workers,
                max_open_managed_mrs: profile.max_open_managed_mrs(),
                validation_timeout_seconds: profile.validation_timeout_seconds(),
                manager_wake_autonomy: match profile.manager_wake_autonomy {
                    WakeAutonomy::Off => "off",
                    WakeAutonomy::ReviewOnly => "review_only",
                    WakeAutonomy::Full => "full",
                },
                delivery_mode: profile.delivery_mode.as_str(),
            }
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string(&summaries)?)
}

#[cfg(test)]
mod tests {
    use super::list_json;
    use crate::config::GahConfig;

    /// The server's chat-session service creates worktrees under
    /// defaults.worktree_base; it learns that base from `gah profile list
    /// --json`, so the field must be present even when TOML leaves it empty.
    #[test]
    fn profile_list_json_exposes_worktree_base() {
        let config: GahConfig = toml::from_str(
            r#"
[defaults]
worktree_base = "/srv/gah/worktrees"

[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"
"#,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&list_json(&config).unwrap()).unwrap();
        let summary = &parsed[0];
        assert_eq!(summary["worktree_base"], "/srv/gah/worktrees");
        assert_eq!(summary["repo_id"], "repo");
    }
}
