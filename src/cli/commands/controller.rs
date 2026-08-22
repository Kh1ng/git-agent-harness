// Command execution for controller-facing `gah` subcommands (ticket #408).

use anyhow::Result;

use crate::cli::args::{HoldCommands, RouteApprovalCommands};
use crate::{config, controller as controller_runtime, events, ledger, runner, status, sync};

pub struct LoopArgs {
    pub profile: String,
    pub config_path: Option<String>,
    pub json: bool,
    pub once: bool,
    pub parallel: usize,
    pub skip_validation_gate: bool,
}

pub struct EventsArgs {
    pub config_path: Option<String>,
    pub profile: Option<String>,
    pub json: bool,
    pub since: String,
}

pub struct StatusArgs {
    pub profile: String,
    pub json: bool,
    pub config_path: Option<String>,
}

pub struct SyncArgs {
    pub profile: String,
    pub config_path: Option<String>,
    pub json: bool,
}

pub fn run_hold(command: HoldCommands) -> Result<()> {
    match command {
        HoldCommands::Set {
            profile,
            work_id,
            reason,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_review_hold(&profile, prof, &work_id, reason);
            let path = ledger::append(&cfg, &entry)?;
            println!(
                "Review hold set for work_id '{}' on profile '{}' ({})",
                work_id,
                profile,
                path.display()
            );
        }
        HoldCommands::Clear {
            profile,
            work_id,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let prof = config::get_profile(&cfg, &profile)?;
            let entry = ledger::LedgerEntry::new_review_hold_release(&profile, prof, &work_id);
            let path = ledger::append(&cfg, &entry)?;
            println!(
                "Review hold cleared for work_id '{}' on profile '{}' ({})",
                work_id,
                profile,
                path.display()
            );
        }
    }
    Ok(())
}

pub fn run_route_approval(command: RouteApprovalCommands) -> Result<()> {
    let (profile, work_id, backend, instance, model, config_path, granted) = match command {
        RouteApprovalCommands::Grant {
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
        } => (
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
            true,
        ),
        RouteApprovalCommands::Revoke {
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
        } => (
            profile,
            work_id,
            backend,
            instance,
            model,
            config_path,
            false,
        ),
    };
    let cfg = config::load(config_path.as_deref())?;
    let prof = config::get_profile(&cfg, &profile)?;
    if let Some(instance_name) = instance.as_deref() {
        let routing = prof.effective_routing(&cfg.defaults);
        let declared = routing
            .backend_instances
            .get(instance_name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown backend instance '{instance_name}' for profile '{profile}'"
                )
            })?;
        let logical_backend = declared
            .logical_backend
            .as_deref()
            .unwrap_or(declared.runner_kind.as_str());
        if logical_backend != backend {
            anyhow::bail!(
                "backend instance '{}' belongs to logical backend '{}', not '{}'",
                instance_name,
                logical_backend,
                backend
            );
        }
    }
    let entry = ledger::LedgerEntry::new_paid_route_approval_for_instance(
        &profile,
        prof,
        &work_id,
        &backend,
        instance.as_deref(),
        model.as_deref(),
        granted,
    );
    let path = ledger::append(&cfg, &entry)?;
    println!(
        "Paid route approval {} for work_id '{}' on {}{}/{} ({})",
        if granted { "granted" } else { "revoked" },
        work_id,
        backend,
        instance
            .as_deref()
            .map(|instance| format!(" [{instance}]"))
            .unwrap_or_default(),
        model.as_deref().unwrap_or("default"),
        path.display()
    );
    Ok(())
}

/// Mirrors the Node server's `loopManualStopFile` path convention
/// (apps/server/src/gahCli.ts): `loopStateDir()/loop-<profile>.manual-stop.json`,
/// where `loopStateDir()` is either `<config-dir>/.gah-locks` (when the server
/// resolved a config path) or `$XDG_STATE_HOME/gah` (fallback). We check both
/// so a marker written by either server configuration is honored.
fn manual_stop_marker_present(profile: &str, config_path: &std::path::Path) -> bool {
    let key = profile.replace('/', "_");
    let mut candidates = Vec::new();
    if let Some(parent) = config_path.parent() {
        candidates.push(
            parent
                .join(".gah-locks")
                .join(format!("loop-{key}.manual-stop.json")),
        );
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        candidates.push(
            std::path::PathBuf::from(xdg)
                .join("gah")
                .join(format!("loop-{key}.manual-stop.json")),
        );
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            std::path::PathBuf::from(home)
                .join(".local/state/gah")
                .join(format!("loop-{key}.manual-stop.json")),
        );
    }
    candidates.into_iter().any(|p| p.is_file())
}

pub fn run_loop(args: LoopArgs) -> Result<()> {
    runner::install_shutdown_handler()?;
    let cfg = config::load(args.config_path.as_deref())?;
    let resolved_config_path = config::resolve_config_path(args.config_path.as_deref());

    // Issue: honor the manual-stop marker at daemon startup. When the
    // operator stops the loop through the control plane (dashboard Stop), the
    // Node server writes `loopStateDir()/loop-<profile>.manual-stop.json` so
    // the stop survives a reboot -- the gah-loop@<profile>.service unit is
    // enabled at default.target and would otherwise auto-start and begin
    // dispatching immediately after every reboot. The loop refuses to start
    // while the marker is present; the operator restarts it explicitly from
    // the dashboard (Start clears the marker before systemctl start).
    if !args.once && manual_stop_marker_present(&args.profile, &resolved_config_path) {
        eprintln!(
            "gah loop: profile '{}' was deliberately stopped through the control plane (manual-stop marker present). \
             Not starting to avoid re-dispatching after reboot. Start it from the dashboard or with \
             `systemctl --user start gah-loop@{}`.",
            args.profile, args.profile
        );
        return Ok(());
    }

    // Issue #881: advisory only -- unset registry_central_url skips this
    // entirely. See crate::fleet_preflight module docs for why it can't
    // (and doesn't try to) guarantee two nodes never dispatch the same
    // profile concurrently.
    if let Some(central_url) = &cfg.defaults.registry_central_url {
        let token = std::env::var("COORDINATOR_TOKEN").ok();
        crate::fleet_preflight::check(
            &crate::fleet_preflight::CurlFleetQuerier,
            central_url,
            &args.profile,
            token.as_deref(),
            cfg.defaults.registry_preflight_mode,
        )?;
    }
    let parallel = controller_runtime::loop_parallel_argument(
        args.once,
        args.parallel,
        config::get_profile(&cfg, &args.profile)?.max_parallel_workers() as usize,
    );
    if args.once {
        // `--once` still does real execution (spawns backends, claims tickets,
        // writes ledger entries) so it must coordinate via the same profile
        // lock as the daemon (`gah loop` with no `--once`).
        let _lock = controller_runtime::acquire_profile_lock(&args.profile, &resolved_config_path)?;
        // Issue #761: a bounded `--once` iteration skips the periodic
        // availability/quota probes (see controller/runtime/probe.rs) --
        // finishes long before their intervals would matter anyway, and
        // several test fixtures spawn `gah loop --once` against a fake
        // codex/vibe binary that reacts to any invocation.
        controller_runtime::run_once(
            &cfg,
            &args.profile,
            args.json,
            parallel,
            args.skip_validation_gate,
            false,
        )?;
    } else {
        controller_runtime::run_loop(
            &cfg,
            &args.profile,
            args.json,
            parallel,
            args.skip_validation_gate,
            &resolved_config_path,
        )?;
    }
    Ok(())
}

pub fn run_events(args: EventsArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    events::run(&cfg, &args.since, args.profile.as_deref(), args.json)?;
    Ok(())
}

pub fn run_status(args: StatusArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    status::run(&cfg, &args.profile, args.json)?;
    Ok(())
}

pub fn run_sync(args: SyncArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    sync::run(&cfg, &args.profile, args.json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::manual_stop_marker_present;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    /// The manual-stop marker paths are env-dependent (XDG_STATE_HOME / HOME),
    /// so tests that mutate them must serialize and restore.
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn with_temp_dir<F: FnOnce(&std::path::Path)>(f: F) {
        let dir = tempfile::tempdir().unwrap();
        f(dir.path());
    }

    #[test]
    fn manual_stop_marker_detects_file_next_to_config_gah_locks() {
        let _guard = env_guard();
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_STATE_HOME");
        std::env::remove_var("HOME");

        with_temp_dir(|dir| {
            let locks = dir.join(".gah-locks");
            std::fs::create_dir_all(&locks).unwrap();
            std::fs::write(
                locks.join("loop-gah.manual-stop.json"),
                r#"{"stoppedAt":"2026-08-22T00:00:00Z"}"#,
            )
            .unwrap();
            let config_path = dir.join("config.toml");
            assert!(manual_stop_marker_present("gah", &config_path));
        });

        restore_env("XDG_STATE_HOME", saved_xdg);
        restore_env("HOME", saved_home);
    }

    #[test]
    fn manual_stop_marker_absent_when_no_marker_file() {
        let _guard = env_guard();
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_STATE_HOME");
        std::env::remove_var("HOME");

        with_temp_dir(|dir| {
            std::fs::create_dir_all(dir.join(".gah-locks")).unwrap();
            let config_path = dir.join("config.toml");
            assert!(!manual_stop_marker_present("gah", &config_path));
        });

        restore_env("XDG_STATE_HOME", saved_xdg);
        restore_env("HOME", saved_home);
    }

    #[test]
    fn manual_stop_marker_detects_file_in_xdg_state_home_gah() {
        let _guard = env_guard();
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("HOME");

        with_temp_dir(|dir| {
            let state = dir.join("gah");
            std::fs::create_dir_all(&state).unwrap();
            std::fs::write(
                state.join("loop-gah.manual-stop.json"),
                r#"{"stoppedAt":"2026-08-22T00:00:00Z"}"#,
            )
            .unwrap();
            std::env::set_var("XDG_STATE_HOME", dir);
            let config_path = dir.join("config.toml");
            assert!(manual_stop_marker_present("gah", &config_path));
        });

        restore_env("XDG_STATE_HOME", saved_xdg);
        restore_env("HOME", saved_home);
    }

    #[test]
    fn manual_stop_marker_ignores_other_profiles() {
        let _guard = env_guard();
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_STATE_HOME");
        std::env::remove_var("HOME");

        with_temp_dir(|dir| {
            let locks = dir.join(".gah-locks");
            std::fs::create_dir_all(&locks).unwrap();
            std::fs::write(
                locks.join("loop-sportsball.manual-stop.json"),
                r#"{"stoppedAt":"2026-08-22T00:00:00Z"}"#,
            )
            .unwrap();
            let config_path = dir.join("config.toml");
            assert!(!manual_stop_marker_present("gah", &config_path));
        });

        restore_env("XDG_STATE_HOME", saved_xdg);
        restore_env("HOME", saved_home);
    }

    fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
