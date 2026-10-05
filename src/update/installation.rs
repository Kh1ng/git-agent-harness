use super::HostRole;
use super::{copy_opencode_agent_configs, install_quota_refresh_unit_template};
use anyhow::{bail, Result};
use std::env;
use std::path::Path;

/// Enumerate installation effects before confirmation, shared with setup.
pub fn installation_plan(role: HostRole, agents: &[String]) -> Result<Vec<String>> {
    for agent in agents {
        if !matches!(agent.as_str(), "claude" | "codex" | "opencode" | "vibe") {
            bail!("unknown installation agent: {agent}");
        }
    }
    let mut plan = vec![
        "Install only gah into $CARGO_HOME/bin (default ~/.cargo/bin); build dependencies and outputs in the checkout".into(),
        "Install gah-loop@.service and gah-watchdog.service/timer in the user systemd directory; enable user lingering via loginctl/sudo".into(),
    ];
    if matches!(role, HostRole::Central | HostRole::Standalone) {
        plan.push("Build server/MCP; use sudo to install /etc/systemd/system/gah-server.service; install and enable user gah-prune.service/timer".into());
        if let Some(root) = super::resolve_web_deploy_root(env::var_os("GAH_WEB_DEPLOY_ROOT"))? {
            plan.push(format!(
                "Build and deploy web UI to {} (sudo; replace index and prune stale assets)",
                root.display()
            ));
        }
    }
    if agents.iter().any(|agent| agent == "opencode") {
        plan.push("Install OpenCode files in the user config directory opencode/agents/".into());
    }
    if quota_refresh_selected(agents) {
        plan.push("Install and enable user gah-quota-refresh.service/timer for Codex/Vibe".into());
    }
    if cfg!(target_os = "macos") {
        plan.push("Install the desktop app and role LaunchAgent under ~/Applications and ~/Library/LaunchAgents".into());
    }
    Ok(plan)
}

pub(super) fn quota_refresh_selected(agents: &[String]) -> bool {
    agents
        .iter()
        .any(|agent| matches!(agent.as_str(), "codex" | "vibe"))
}

pub(super) fn install_selected_agent_assets(
    repo: &Path,
    config_home: &Path,
    agents: &[String],
) -> Result<()> {
    if agents.iter().any(|agent| agent == "opencode") {
        for agent in copy_opencode_agent_configs(repo, config_home)? {
            println!("Installed OpenCode agent: {}", agent.display());
        }
    }
    if quota_refresh_selected(agents) {
        install_quota_refresh_unit_template(repo)?;
    }
    Ok(())
}
