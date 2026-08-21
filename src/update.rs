//! Deterministic CLI and control-plane server update workflow.
//!
//! `cargo build --release` only updates `target/release/gah`. The command a
//! host actually invokes normally lives at `$CARGO_HOME/bin/gah`, so a normal
//! build can silently leave the control plane on old behavior.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::env;
use std::fs::{copy, create_dir_all, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether this host runs the control plane (`apps/server`, `gah-server.service`)
/// or is just a worker that dispatches jobs. A worker never needs the Node
/// server built or started -- only the CLI and (when systemd is present) the
/// dispatch-loop unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostRole {
    Central,
    Worker,
}

impl HostRole {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "central" => Ok(HostRole::Central),
            "worker" => Ok(HostRole::Worker),
            other => bail!("invalid --role '{other}' (expected 'central' or 'worker')"),
        }
    }
}

pub struct UpdateArgs {
    pub repo: Option<PathBuf>,
    pub role: HostRole,
    pub restart_server: bool,
    pub server_service: String,
}

pub fn run(args: UpdateArgs) -> Result<()> {
    if args.role == HostRole::Worker && args.restart_server {
        bail!("--restart-server requires --role central; a worker never runs gah-server.service");
    }

    let repo = resolve_repo(args.repo.as_deref())?;
    let _update_lock = acquire_update_lock(&repo)?;
    ensure_default_branch_checkout(&repo)?;
    ensure_clean(&repo)?;
    if args.restart_server {
        ensure_no_running_loop_before_server_restart()?;
    }

    println!("Updating GAH CLI/control plane from {}", repo.display());
    run_command(&repo, "git", &["fetch", "origin", "--prune"])?;
    run_command(&repo, "git", &["pull", "--ff-only"])?;

    // This is the authoritative CLI deployment step. It replaces the Cargo
    // executable selected by PATH, unlike a target/release-only build.
    run_command(&repo, "cargo", &["install", "--path", ".", "--force"])?;

    let binary = installed_binary_path()?;
    if !binary.is_file() {
        bail!(
            "cargo install completed but expected executable is missing: {}",
            binary.display()
        );
    }
    run_command(&repo, binary.to_string_lossy().as_ref(), &["--help"])?;
    println!("Installed CLI: {}", binary.display());

    if args.role == HostRole::Central {
        // The control-plane server is part of the MVP; web/desktop/mobile
        // clients intentionally have independent release workflows. A
        // worker node dispatches jobs only and never serves this.
        run_command(
            &repo,
            "npm",
            &[
                "ci",
                "--include=dev",
                "--legacy-peer-deps",
                "--prefer-offline",
                "--no-audit",
                "--no-fund",
            ],
        )?;
        run_command(&repo, "npm", &["run", "build:server"])?;
        if !repo.join("apps/server/dist/bin.js").is_file() {
            bail!("server build did not produce apps/server/dist/bin.js");
        }
        println!(
            "Built server:  {}",
            repo.join("apps/server/dist/bin.js").display()
        );

        // Issue #894: keep the system-level control-plane unit in lockstep
        // with the installed server, exactly like the user units below. The
        // gah-server.service is a system unit (not user), so it needs sudo.
        // Operator customization belongs in `systemctl edit gah-server.service`
        // drop-ins; replacing the base template on every update is what
        // prevents the stale-unit drift this fix exists for.
        match install_server_unit_template(&repo, &args.server_service)? {
            Some(target) => println!("Installed server unit: {}", target.display()),
            None => {
                println!("systemd not available on this host: skipping gah-server.service install.")
            }
        }

        // Issue #896: build the web dashboard and deploy it to wherever the
        // host's web server (Caddy, by default) serves it from. The deploy
        // root is configurable via GAH_WEB_DEPLOY_ROOT; unset (the default)
        // deploys to /var/www/gah, an explicitly empty value skips deploy.
        match deploy_web_ui(&repo)? {
            Some(root) => println!("Deployed web UI to {}", root.display()),
            None => println!("GAH_WEB_DEPLOY_ROOT is empty: skipping web UI deploy."),
        }
    } else {
        println!("Role is 'worker': skipping control-plane server build.");
    }

    match install_loop_unit_template(&repo)? {
        Some(loop_unit) => println!("Installed loop unit: {}", loop_unit.display()),
        None => println!(
            "systemd not available on this host: skipping gah-loop@.service install. \
             Run `gah loop --profile <p>` directly, or wire it into whatever this host \
             uses for supervised long-running processes (e.g. launchd on macOS)."
        ),
    }
    match install_watchdog_unit_template(&repo)? {
        Some(watchdog_units) => {
            for unit in &watchdog_units {
                println!("Installed watchdog unit: {}", unit.display());
            }
            println!(
                "Watchdog timer is installed but not enabled; opt in explicitly with \
                 `systemctl --user enable --now gah-watchdog.timer` once an alert command is configured."
            );
        }
        None => println!("systemd not available on this host: skipping watchdog unit install."),
    }

    if args.restart_server {
        run_command(&repo, "sudo", &["systemctl", "daemon-reload"])?;
        run_command(
            &repo,
            "sudo",
            &["systemctl", "restart", &args.server_service],
        )?;
        run_command(
            &repo,
            "systemctl",
            &["is-active", "--quiet", &args.server_service],
        )?;
        println!("Restarted service: {}", args.server_service);
    } else if args.role == HostRole::Central {
        println!(
            "Server not restarted; pass --restart-server when this host serves the control plane."
        );
    }
    Ok(())
}

/// Best-effort probe, not a hard dependency check: a missing `systemctl`
/// (e.g. macOS, containers without systemd) means unit installation is
/// skipped rather than failing the whole update.
fn systemd_available() -> bool {
    Command::new("systemctl")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn resolve_repo(repo: Option<&Path>) -> Result<PathBuf> {
    let path = repo
        .map(Path::to_path_buf)
        .unwrap_or(env::current_dir().context("reading current directory")?);
    let output = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("resolving repository root")?;
    if !output.status.success() {
        bail!(
            "{} is not a Git checkout: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(PathBuf::from(
        String::from_utf8(output.stdout)
            .context("repository root was not UTF-8")?
            .trim(),
    ))
}

fn ensure_default_branch_checkout(repo: &Path) -> Result<()> {
    let branch = captured(repo, "git", &["branch", "--show-current"])?;
    let default_ref = captured(
        repo,
        "git",
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "cannot determine origin's default branch; run `git remote set-head origin -a` in {} before updating",
            repo.display()
        )
    })?;
    let default_branch = default_ref.strip_prefix("origin/").unwrap_or(&default_ref);
    if branch != default_branch {
        bail!(
            "refusing to update non-default branch '{branch}' (origin default is '{default_branch}'); switch to the default branch first"
        );
    }
    Ok(())
}

/// Serialize all mutable update steps (`git pull`, `cargo install`, and `npm
/// ci`) for one checkout. Unlike a profile lock this is deliberately repo-wide:
/// two operators updating the same source tree would otherwise race on Git and
/// dependency state before any GAH profile exists.
fn acquire_update_lock(repo: &Path) -> Result<File> {
    let git_dir = captured(repo, "git", &["rev-parse", "--absolute-git-dir"])?;
    let lock_path = PathBuf::from(git_dir).join("gah-update.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening update lock {}", lock_path.display()))?;
    lock.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "another `gah update` is already running for {}; wait for it to finish",
            repo.display()
        )
    })?;
    Ok(lock)
}

/// The loop is an independent systemd user unit. Refuse a server restart
/// while one is active so an update cannot change the control plane beneath
/// ongoing work; the operator must intentionally stop it first.
fn ensure_no_running_loop_before_server_restart() -> Result<()> {
    let output = Command::new("pgrep")
        .args(["-af", "gah loop --profile"])
        .output()
        .context("checking for active gah loops before server restart")?;
    if output.status.code() == Some(1) {
        return Ok(());
    }
    if !output.status.success() {
        bail!(
            "could not determine whether a gah loop is active before restarting the server: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let processes = String::from_utf8_lossy(&output.stdout).trim().to_string();
    bail!(
        "refusing to restart gah-server.service while a gah loop is active:\n{processes}\nStop the loop cleanly from the dashboard or `gah loop` owner, then rerun `gah update --restart-server`."
    );
}

/// Keep the dashboard's lifecycle unit in lockstep with the installed CLI and
/// control plane. Local operator customization belongs in `systemctl --user
/// edit gah-loop@<profile>` drop-ins, so replacing this base template on every
/// deterministic update is safe and prevents source/runtime ownership drift.
fn systemd_user_config_home() -> Result<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("HOME or XDG_CONFIG_HOME is required to install a systemd user unit")
}

fn copy_systemd_unit(repo: &Path, config_home: &Path, unit_file_name: &str) -> Result<PathBuf> {
    let source = repo.join("packaging/systemd").join(unit_file_name);
    if !source.is_file() {
        bail!("systemd unit template is missing: {}", source.display());
    }
    let target = config_home.join("systemd/user").join(unit_file_name);
    let parent = target.parent().expect("systemd unit target has a parent");
    create_dir_all(parent)
        .with_context(|| format!("creating systemd user-unit directory {}", parent.display()))?;
    copy(&source, &target).with_context(|| {
        format!(
            "installing systemd unit from {} to {}",
            source.display(),
            target.display()
        )
    })?;
    Ok(target)
}

/// Issue #894: keep the system-level control-plane unit in lockstep with the
/// installed server, same reasoning as `install_loop_unit_template`. Unlike
/// the user units, gah-server.service is a system unit owned by root, so
/// install needs `sudo`; operator customization belongs in `systemctl edit
/// gah-server.service` drop-ins, so replacing the base template on every
/// deterministic update is safe. `--server-service` names the installed unit
/// (default `gah-server.service`), so the copy target matches what the
/// restart step below restarts.
fn install_server_unit_template(repo: &Path, server_service: &str) -> Result<Option<PathBuf>> {
    if !systemd_available() {
        return Ok(None);
    }
    let source = repo.join("packaging/systemd/gah-server.service");
    if !source.is_file() {
        bail!("systemd unit template is missing: {}", source.display());
    }
    let target = PathBuf::from("/etc/systemd/system").join(server_service);
    run_command(
        repo,
        "sudo",
        &[
            "install",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            "0644",
            source.to_str().unwrap_or_default(),
            target.to_str().unwrap_or_default(),
        ],
    )?;
    run_command(repo, "sudo", &["systemctl", "daemon-reload"])?;
    Ok(Some(target))
}

/// Issue #896: build `apps/web` and deploy its `dist` to wherever the host's
/// web server serves the dashboard from. The deploy root is configurable via
/// `GAH_WEB_DEPLOY_ROOT`:
///
/// - unset -> `/var/www/gah` (the Caddy root the repo documents/ships)
/// - set to a non-empty path -> that path
/// - set to empty -> skip deployment entirely
///
/// The web root is typically root-owned (Caddy's static site), so copying
/// needs `sudo`. Deployment is a replace-in-place of the dist contents, not
/// a delete of the whole root -- operator-provided files (favicon overrides,
/// robots.txt, a Caddyfile) survive the update.
fn deploy_web_ui(repo: &Path) -> Result<Option<PathBuf>> {
    let root = env::var("GAH_WEB_DEPLOY_ROOT").unwrap_or_else(|_| "/var/www/gah".to_string());
    if root.trim().is_empty() {
        return Ok(None);
    }
    run_command(repo, "npm", &["run", "build:web"])?;
    let dist = repo.join("apps/web/dist");
    if !dist.join("index.html").is_file() {
        bail!("web build did not produce apps/web/dist/index.html");
    }
    let root_path = PathBuf::from(&root);
    run_command(
        repo,
        "sudo",
        &[
            "install", "-d", "-o", "root", "-g", "root", "-m", "0755", &root,
        ],
    )?;
    // Copy the dist contents into the root. `dist/` itself is not copied as
    // a nested directory; trailing separator semantics are fiddly across
    // BSD/Linux cp, so use an explicit glob-style loop via cp -r of the
    // directory contents.
    run_command(
        repo,
        "sudo",
        &[
            "sh",
            "-c",
            &format!("cp -r '{}'/. '{}'", dist.display(), root),
        ],
    )?;
    Ok(Some(root_path))
}

fn reload_user_systemd(reason: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .with_context(|| format!("reloading the user systemd manager after {reason}"))?;
    if !status.success() {
        bail!("systemctl --user daemon-reload exited with {status}");
    }
    Ok(())
}

fn install_loop_unit_template(repo: &Path) -> Result<Option<PathBuf>> {
    if !systemd_available() {
        return Ok(None);
    }
    let config_home = systemd_user_config_home()?;
    let target = copy_systemd_unit(repo, &config_home, "gah-loop@.service")?;
    reload_user_systemd("installing gah-loop@.service")?;
    Ok(Some(target))
}

/// Issue #726: keep the alert-only watchdog unit in lockstep with the
/// installed CLI, same reasoning as `install_loop_unit_template`. This also
/// safely replaces an old host-local-script-based `gah-watchdog.service`
/// left over from before this unit was tracked in-repo (AC7) -- the copy
/// destination is identical, so a stale version is simply overwritten.
/// Deliberately does not `enable` or `start` the timer: that stays an
/// explicit operator opt-in.
fn install_watchdog_unit_template(repo: &Path) -> Result<Option<[PathBuf; 2]>> {
    if !systemd_available() {
        return Ok(None);
    }
    let config_home = systemd_user_config_home()?;
    let service = copy_systemd_unit(repo, &config_home, "gah-watchdog.service")?;
    let timer = copy_systemd_unit(repo, &config_home, "gah-watchdog.timer")?;
    reload_user_systemd("installing gah-watchdog.service/.timer")?;
    Ok(Some([service, timer]))
}

fn ensure_clean(repo: &Path) -> Result<()> {
    let status = captured(repo, "git", &["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!(
            "refusing to update a dirty checkout; commit, stash, or move these changes first:\n{status}"
        );
    }
    Ok(())
}

fn installed_binary_path() -> Result<PathBuf> {
    let cargo_home = match env::var_os("CARGO_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            env::var_os("HOME").context("HOME is required to locate Cargo-installed gah")?,
        )
        .join(".cargo"),
    };
    Ok(cargo_home.join("bin").join("gah"))
}

fn captured(repo: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("starting {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .context("command output was not UTF-8")
}

fn run_command(repo: &Path, program: &str, args: &[&str]) -> Result<()> {
    println!("> {} {}", program, args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(repo)
        .status()
        .with_context(|| format!("starting {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} exited with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        deploy_web_ui, ensure_clean, ensure_default_branch_checkout, install_server_unit_template,
        install_watchdog_unit_template, installed_binary_path, run, HostRole, UpdateArgs,
    };
    use crate::test_support::PathGuard;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static XDG_CONFIG_HOME_LOCK: Mutex<()> = Mutex::new(());

    /// Scoped override for `XDG_CONFIG_HOME`, mirroring the other process-env
    /// guards in `crate::test_support` -- must be restored before another
    /// parallel test can observe process state.
    struct XdgConfigHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        original: Option<OsString>,
    }

    impl XdgConfigHomeGuard {
        fn set(path: impl AsRef<std::ffi::OsStr>) -> Self {
            let lock = XDG_CONFIG_HOME_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let original = std::env::var_os("XDG_CONFIG_HOME");
            std::env::set_var("XDG_CONFIG_HOME", path);
            Self {
                _lock: lock,
                original,
            }
        }
    }

    impl Drop for XdgConfigHomeGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository_with_origin_head() -> (TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        (tmp, repo)
    }

    #[test]
    fn installed_binary_lives_in_cargo_bin() {
        let path = installed_binary_path().unwrap();
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("gah"));
        assert_eq!(
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            Some("bin")
        );
    }

    #[test]
    fn default_branch_check_accepts_origin_default_and_rejects_feature_branch() {
        let (_tmp, repo) = repository_with_origin_head();
        ensure_default_branch_checkout(&repo).unwrap();
        git(&repo, &["checkout", "-b", "feature"]);
        let err = ensure_default_branch_checkout(&repo).unwrap_err();
        assert!(err.to_string().contains("non-default branch 'feature'"));
    }

    #[test]
    fn default_branch_check_explains_missing_origin_head() {
        let (_tmp, repo) = repository_with_origin_head();
        git(&repo, &["symbolic-ref", "-d", "refs/remotes/origin/HEAD"]);
        let err = ensure_default_branch_checkout(&repo).unwrap_err();
        assert!(err.to_string().contains("git remote set-head origin -a"));
    }

    #[test]
    fn clean_check_rejects_uncommitted_changes() {
        let (_tmp, repo) = repository_with_origin_head();
        ensure_clean(&repo).unwrap();
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
        let err = ensure_clean(&repo).unwrap_err();
        assert!(err.to_string().contains("dirty.txt"));
    }

    /// Issue #726 AC1/AC7: `gah update` installs the watchdog unit
    /// deterministically and reloads the user systemd manager, but never
    /// enables or starts anything -- only `daemon-reload` may appear in the
    /// recorded systemctl invocations.
    #[test]
    fn watchdog_unit_template_is_installed_without_enabling_or_starting_anything() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config_tmp = TempDir::new().unwrap();
        let _xdg_guard = XdgConfigHomeGuard::set(config_tmp.path());

        let bin_tmp = TempDir::new().unwrap();
        let record_path = bin_tmp.path().join("argv.log");
        let script = format!("#!/bin/sh\necho \"$@\" >> '{}'\n", record_path.display());
        let script_path = bin_tmp.path().join("systemctl");
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms).unwrap();
        }
        let _path_guard = PathGuard::set(bin_tmp.path().to_str().unwrap());

        let [service, timer] = install_watchdog_unit_template(repo).unwrap().unwrap();

        assert!(service.ends_with("systemd/user/gah-watchdog.service"));
        assert!(timer.ends_with("systemd/user/gah-watchdog.timer"));
        assert!(
            std::fs::read_to_string(&service)
                .unwrap()
                .contains("gah watchdog-check"),
            "installed unit should invoke the packaged watchdog-check command"
        );
        assert!(
            !std::fs::read_to_string(&service)
                .unwrap()
                .contains("/home/khing/workspace/agent-lab"),
            "installed unit must not reference the old host-local script path"
        );

        let record = std::fs::read_to_string(&record_path).unwrap();
        assert!(!record.is_empty(), "expected systemctl to be invoked");
        for forbidden in ["start", "restart", "enable"] {
            assert!(
                !record.split_whitespace().any(|arg| arg == forbidden),
                "systemctl was invoked with forbidden verb '{forbidden}': {record}"
            );
        }
        assert!(record.contains("daemon-reload"), "{record}");
    }

    #[test]
    fn host_role_parses_only_known_values() {
        assert_eq!(HostRole::parse("central").unwrap(), HostRole::Central);
        assert_eq!(HostRole::parse("worker").unwrap(), HostRole::Worker);
        assert!(HostRole::parse("bogus").is_err());
    }

    /// A worker node never runs `gah-server.service` -- `--restart-server`
    /// on `--role worker` is a contradiction, not something to silently
    /// ignore or default away.
    #[test]
    fn worker_role_rejects_restart_server_flag() {
        let err = run(UpdateArgs {
            repo: None,
            role: HostRole::Worker,
            restart_server: true,
            server_service: "gah-server.service".into(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("--role central"));
    }

    /// Issue #894: `gah update` (central role) reinstalls the system-level
    /// `gah-server.service` unit from the tracked template on every run, so
    /// the installed unit can never drift from `packaging/systemd/`. The fake
    /// `sudo` shim records the invocation; the assertion checks the template
    /// was copied to /etc/systemd/system and daemon-reload ran.
    #[test]
    fn server_unit_template_is_installed_on_every_update() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let bin_tmp = TempDir::new().unwrap();
        let record_path = bin_tmp.path().join("sudo-argv.log");
        // Fake `sudo` that just logs its args and forwards to the real
        // `install`/`systemctl` is too fragile; instead log and succeed so
        // the test asserts the *plan* of the update, not the root-owned copy.
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nif [ \"$1\" = \"install\" ]; then exit 0; fi\nif [ \"$1\" = \"systemctl\" ] && [ \"$2\" = \"daemon-reload\" ]; then exit 0; fi\nexit 0\n",
            record_path.display()
        );
        let script_path = bin_tmp.path().join("sudo");
        std::fs::write(&script_path, script).unwrap();
        // systemd_available() runs `systemctl --version`; without a fake
        // systemctl on PATH the unit install is skipped entirely (returns
        // None) and this test can't assert anything.
        let systemctl_path = bin_tmp.path().join("systemctl");
        std::fs::write(
            &systemctl_path,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'systemd 255'; exit 0; fi\nif [ \"$1\" = \"daemon-reload\" ]; then exit 0; fi\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&script_path, &systemctl_path] {
                let mut perms = std::fs::metadata(path).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(path, perms).unwrap();
            }
        }
        let _path_guard = PathGuard::set(bin_tmp.path().to_str().unwrap());

        let target = install_server_unit_template(repo, "gah-server.service")
            .unwrap()
            .unwrap();
        assert!(target.ends_with("/etc/systemd/system/gah-server.service"));
        let record = std::fs::read_to_string(&record_path).unwrap();
        assert!(
            record.contains("install"),
            "expected sudo install: {record}"
        );
        assert!(record.contains("gah-server.service"), "{record}");
        assert!(record.contains("/etc/systemd/system"), "{record}");
        assert!(record.contains("daemon-reload"), "{record}");
    }

    /// Issue #896: an explicitly-empty GAH_WEB_DEPLOY_ROOT skips deployment
    /// (returns None) without touching npm or the filesystem -- the operator
    /// opted out of web deploy for this host.
    #[test]
    fn empty_web_deploy_root_skips_deployment() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let previous = std::env::var_os("GAH_WEB_DEPLOY_ROOT");
        std::env::set_var("GAH_WEB_DEPLOY_ROOT", "");
        let result = deploy_web_ui(repo).unwrap();
        match previous {
            Some(value) => std::env::set_var("GAH_WEB_DEPLOY_ROOT", value),
            None => std::env::remove_var("GAH_WEB_DEPLOY_ROOT"),
        }
        assert!(result.is_none());
    }
}
