//! Render tracked systemd templates for the account that installs them (#1322).
//! Templates carry `@USER@`, `@REPO@`, `@CONFIG@`, `@NODE@`, `@GAH@` and
//! `@PATH@` placeholders; nothing developer-specific is tracked.
use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) struct UnitValues {
    user: Option<String>,
    repo: PathBuf,
    config: PathBuf,
    node: Option<PathBuf>,
    gah: PathBuf,
    path: String,
}

impl UnitValues {
    /// Refuse to render or deploy for root: the units must describe the
    /// invoking account, and root's HOME/config reproduce the #1322 failure.
    /// `SUDO_USER` survives `sudo -iu <account>`, so the check is the
    /// effective uid, not sudo's environment.
    pub(super) fn ensure_installing_account() -> Result<()> {
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            bail!(
                "run gah update without sudo; GAH requests sudo only for system unit installation"
            );
        }
        Ok(())
    }

    /// Resolve every value from the running account and its PATH, the same
    /// environment that just built the server.
    pub(super) fn resolve(repo: &Path) -> Result<Self> {
        Self::ensure_installing_account()?;
        let user = Command::new("id")
            .arg("-un")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|user| user.trim().to_string());
        let home = PathBuf::from(env::var_os("HOME").context("HOME is required")?);
        let gah = super::installed_binary_path()?;
        let node = find_on_path("node");
        let mut dirs: Vec<PathBuf> = Vec::new();
        dirs.extend(
            node.as_ref()
                .and_then(|node| node.parent())
                .map(Path::to_path_buf),
        );
        dirs.extend(gah.parent().map(Path::to_path_buf));
        dirs.push(home.join(".opencode/bin"));
        dirs.push(home.join(".local/bin"));
        dirs.extend(
            [
                "/usr/local/sbin",
                "/usr/local/bin",
                "/usr/sbin",
                "/usr/bin",
                "/sbin",
                "/bin",
            ]
            .map(PathBuf::from),
        );
        let mut path = Vec::new();
        for dir in dirs {
            let dir = text(&dir)?;
            if dir.contains(':') {
                bail!("toolchain directory cannot contain ':': {dir}");
            }
            if !path.contains(&dir) {
                path.push(dir);
            }
        }
        Ok(Self {
            user,
            repo: repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf()),
            config: crate::config::resolve_config_path(None),
            node,
            gah,
            path: path.join(":"),
        })
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

/// systemd unquotes `"`/`\` and expands `$` in Exec lines, so those are
/// rejected rather than escaped; `%` specifiers are escaped everywhere.
fn unit_value(name: &str, value: &str) -> Result<String> {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\' | '$'))
    {
        bail!("cannot write {name} into a systemd unit: {value:?}");
    }
    Ok(value.replace('%', "%%"))
}

pub(super) fn render(template: &str, values: &UnitValues) -> Result<String> {
    let absolute = |name: &str, path: &Path| -> Result<String> {
        if !path.is_absolute() {
            bail!("{name} must be an absolute path: {}", path.display());
        }
        unit_value(name, &text(path)?)
    };
    let mut rendered = template.to_string();
    if rendered.contains("@USER@") {
        let user = values
            .user
            .as_deref()
            .context("the installing account could not be resolved with id -un")?;
        if !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            bail!("unsupported service user name: {user:?}");
        }
        rendered = rendered.replace("@USER@", &unit_value("user", user)?);
    }
    if rendered.contains("@NODE@") {
        let node = values.node.as_deref().context(
            "Node.js was not found on PATH; install Node.js 22 or newer, then rerun gah update",
        )?;
        rendered = rendered.replace("@NODE@", &absolute("Node executable", node)?);
    }
    for (placeholder, name, path) in [
        ("@REPO@", "checkout", &values.repo),
        ("@CONFIG@", "config path", &values.config),
        ("@GAH@", "gah executable", &values.gah),
    ] {
        if rendered.contains(placeholder) {
            rendered = rendered.replace(placeholder, &absolute(name, path)?);
        }
    }
    Ok(rendered.replace("@PATH@", &unit_value("PATH", &values.path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(root: &str) -> UnitValues {
        UnitValues {
            user: Some("tester".into()),
            repo: PathBuf::from(format!("{root}/src/gah")),
            config: PathBuf::from(format!("{root}/.config/gah/config.toml")),
            node: Some(PathBuf::from(format!("{root}/node 22/bin/node"))),
            gah: PathBuf::from(format!("{root}/.cargo/bin/gah")),
            path: format!("{root}/node 22/bin:{root}/.cargo/bin:/usr/bin"),
        }
    }

    #[test]
    fn tracked_templates_render_for_another_account() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/systemd");
        let values = values("/srv/other user");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let template = std::fs::read_to_string(&path).unwrap();
            for developer_value in [
                "kh1ng",
                "khing",
                "/home/",
                ".nvm/versions",
                "%h/.cargo/bin/gah",
                "/usr/local/bin/hermes",
            ] {
                assert!(
                    !template.to_lowercase().contains(developer_value),
                    "{} hardcodes {developer_value}",
                    path.display()
                );
            }
            let rendered = render(&template, &values).unwrap();
            assert!(!rendered.contains("@USER@") && !rendered.contains("@PATH@"));
            assert!(!rendered.contains("@NODE@") && !rendered.contains("@REPO@"));
            assert!(!rendered.contains("@CONFIG@") && !rendered.contains("@GAH@"));
        }
    }

    #[test]
    fn resolves_runtime_paths_from_the_installing_environment() {
        let _exec = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let node = dir.path().join("node");
        std::fs::write(&node, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _path = crate::test_support::PathGuard::set(dir.path());
        let resolved = UnitValues::resolve(dir.path()).unwrap();
        assert_eq!(resolved.repo, dir.path().canonicalize().unwrap());
        assert_eq!(resolved.node.as_deref(), Some(node.as_path()));
        assert_eq!(resolved.gah, super::super::installed_binary_path().unwrap());
        assert_eq!(resolved.config, crate::config::resolve_config_path(None));
        assert!(resolved.user.is_some());
        assert_eq!(
            env::split_paths(&resolved.path).next().as_deref(),
            Some(dir.path())
        );
        assert!(find_on_path("gah-uninstalled-test-binary").is_none());
    }

    #[test]
    fn cli_units_use_the_resolved_binary() {
        let mut values = values("/srv/tester");
        values.gah = PathBuf::from("/opt/custom cargo/bin/gah");
        for name in [
            "gah-loop@.service",
            "gah-quota-refresh.service",
            "gah-watchdog.service",
            "gah-prune.service",
        ] {
            let template = std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("packaging/systemd")
                    .join(name),
            )
            .unwrap();
            let rendered = render(&template, &values).unwrap();
            assert!(
                rendered.contains("ExecStart=\"/opt/custom cargo/bin/gah\""),
                "{name}"
            );
        }
    }

    #[test]
    fn server_unit_uses_the_installing_account_and_quotes_spaces() {
        let template = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/systemd/gah-server.service"),
        )
        .unwrap();
        let rendered = render(&template, &values("/srv/other user")).unwrap();
        assert!(rendered.contains("\nUser=tester\n"));
        assert!(rendered.contains("\nWorkingDirectory=/srv/other user/src/gah\n"));
        assert!(rendered.contains(
            "\nEnvironment=\"GAH_CONFIG_PATH=/srv/other user/.config/gah/config.toml\"\n"
        ));
        assert!(rendered.contains("\nExecStart=\"/srv/other user/node 22/bin/node\" apps/server"));
        assert!(rendered.contains("\nEnvironment=\"PATH=/srv/other user/node 22/bin:"));
    }

    #[test]
    fn unsafe_values_are_rejected_and_specifiers_escaped() {
        let mut unsafe_values = values("/srv/a%b");
        assert!(render("WorkingDirectory=@REPO@", &unsafe_values)
            .unwrap()
            .ends_with("/srv/a%%b/src/gah"));
        for repo in [
            "/srv/a\"b",
            "/srv/a\nExecStartPre=/bin/sh",
            "/srv/$HOME",
            "relative",
        ] {
            unsafe_values.repo = PathBuf::from(repo);
            assert!(
                render("WorkingDirectory=@REPO@", &unsafe_values).is_err(),
                "{repo}"
            );
        }
        unsafe_values.user = Some("root\nUser=x".into());
        assert!(render("User=@USER@", &unsafe_values).is_err());
        unsafe_values.node = None;
        let error = render("ExecStart=\"@NODE@\"", &unsafe_values).unwrap_err();
        assert!(error.to_string().contains("Node.js 22"));
        // Templates without Node still render on hosts without it.
        assert!(render("Environment=\"PATH=@PATH@\"", &unsafe_values).is_ok());
    }

    // `sudo -iu <account> gah update` runs as that account while keeping
    // SUDO_USER set, so the guard must key on the effective uid instead.
    static SUDO_USER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct SudoUserEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        original: Option<std::ffi::OsString>,
    }

    impl SudoUserEnvGuard {
        fn set(value: &str) -> Self {
            let _lock = SUDO_USER_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let original = env::var_os("SUDO_USER");
            env::set_var("SUDO_USER", value);
            Self { _lock, original }
        }
    }

    impl Drop for SudoUserEnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => env::set_var("SUDO_USER", value),
                None => env::remove_var("SUDO_USER"),
            }
        }
    }

    #[test]
    fn resolves_for_a_non_root_account_with_sudo_user_set() {
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            // A root test run cannot prove the non-root path this guards.
            return;
        }
        let _sudo_user = SudoUserEnvGuard::set("provisioning-admin");
        let _exec = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        assert!(UnitValues::resolve(dir.path()).is_ok());
    }
}
