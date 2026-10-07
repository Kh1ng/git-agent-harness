use super::HostRole;
use super::{copy_opencode_agent_configs, install_quota_refresh_unit_template};
use anyhow::{bail, Result};
use std::env;
use std::path::Path;

/// `npm ci` arguments shared by the source build path and the release
/// path's dependency-drift reinstall (issue #1416): the release bundle
/// normally ships prebuilt `dist/` output, but when its lockfile differs
/// from the checkout's the installed node_modules must be refreshed too.
pub(super) const NPM_CI_ARGS: &[&str] = &[
    "ci",
    "--include=dev",
    "--legacy-peer-deps",
    "--prefer-offline",
    "--no-audit",
    "--no-fund",
];

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
        plan.push(
            match super::resolve_web_deploy_root(env::var_os("GAH_WEB_DEPLOY_ROOT"))? {
                Some(root) => format!(
                    "Build and deploy web UI to {} (sudo; replace index and prune stale assets)",
                    root.display()
                ),
                None => "Build web UI in the checkout; gah-server serves it".into(),
            },
        );
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

/// Honor explicit selections; otherwise discover installed integrations for
/// callers such as the dashboard updater and older installations.
pub(super) fn agents_to_refresh(config_home: &Path, requested: &[String]) -> Vec<String> {
    if !requested.is_empty() {
        return requested.to_vec();
    }
    let mut agents = requested.to_vec();
    if ["gah-reviewer.md", "gah-implementer.md"]
        .iter()
        .any(|name| config_home.join("opencode/agents").join(name).is_file())
        && !agents.iter().any(|agent| agent == "opencode")
    {
        agents.push("opencode".into());
    }
    if ["gah-quota-refresh.service", "gah-quota-refresh.timer"]
        .iter()
        .any(|name| config_home.join("systemd/user").join(name).is_file())
        && !quota_refresh_selected(&agents)
    {
        // Codex and Vibe share these assets. This selects the refresh action;
        // it does not infer which backend the operator uses.
        agents.push("codex".into());
    }
    agents
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

/// Print the plan and, unless `--yes`, ask before changing anything.
pub(super) fn confirm(plan: &[String], args: &super::UpdateArgs, repo: &Path) -> Result<()> {
    for change in plan {
        println!("  - {change}");
    }
    if args.restart_server {
        println!("  - Restart control-plane service {}", args.server_service);
    }
    if args.pull {
        println!(
            "  - Fetch origin and pull --ff-only into {}",
            repo.display()
        );
    }
    if !args.yes {
        use std::io::Write;
        print!("Apply these changes? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            bail!("Update cancelled before installation");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{agents_to_refresh, install_selected_agent_assets};
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn explicit_agents_do_not_refresh_unselected_existing_integrations() {
        let config = TempDir::new().unwrap();
        let assets = [
            "opencode/agents/gah-reviewer.md",
            "opencode/agents/gah-implementer.md",
            "systemd/user/gah-quota-refresh.service",
            "systemd/user/gah-quota-refresh.timer",
        ];
        for asset in assets {
            let path = config.path().join(asset);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "existing asset").unwrap();
        }
        for agent in ["claude", "codex", "vibe", "opencode"] {
            let requested = vec![agent.to_string()];
            assert_eq!(agents_to_refresh(config.path(), &requested), requested);
        }
        let agents = agents_to_refresh(config.path(), &["claude".into()]);
        install_selected_agent_assets(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            config.path(),
            &agents,
        )
        .unwrap();
        for asset in assets {
            assert_eq!(
                std::fs::read_to_string(config.path().join(asset)).unwrap(),
                "existing asset"
            );
        }
        assert_eq!(
            agents_to_refresh(config.path(), &[]),
            vec!["opencode".to_string(), "codex".to_string()]
        );
    }
}
