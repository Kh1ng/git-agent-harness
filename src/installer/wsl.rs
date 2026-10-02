//! The files `scripts/install-wsl-worker.sh` writes inside WSL: the worker's
//! identity, its private environment, start and register scripts, and a
//! systemd user unit. Credentials go in the environment file, never in a
//! command line or the unit.

use super::files::{replace, shell_quote};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    /// settings.json from the Windows installer: token, central_url,
    /// display_name, advertised_url.
    #[arg(long)]
    pub settings: PathBuf,
    /// The worker's install directory.
    #[arg(long)]
    pub root: PathBuf,
    /// The unpacked release (bin/gah, apps/server/dist).
    #[arg(long)]
    pub release: PathBuf,
    /// The node binary the worker runs on.
    #[arg(long)]
    pub node: PathBuf,
}

#[derive(serde::Deserialize)]
struct Settings {
    token: String,
    central_url: String,
    display_name: String,
    advertised_url: String,
}

/// Double-quoted for systemd, where `%` starts a specifier.
fn systemd_quote(path: &Path) -> String {
    let text = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{text}\"")
}

pub fn install(args: &Args, home: &Path) -> Result<()> {
    let settings: Settings = serde_json::from_str(
        &std::fs::read_to_string(&args.settings)
            .with_context(|| format!("reading {}", args.settings.display()))?,
    )
    .context("reading the worker settings")?;
    super::files::check_value("COORDINATOR_TOKEN", &settings.token)?;
    let config = home.join(".config/gah/config.toml");
    if !config.exists() {
        replace(&config, b"[defaults]\n\n[profiles]\n", 0o644)?;
    }

    let identity_path = args.root.join("identity.json");
    let mut identity = std::fs::read_to_string(&identity_path)
        .ok()
        .and_then(|text| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text).ok()
        })
        .unwrap_or_default();
    if identity
        .get("node_id")
        .and_then(|v| v.as_str())
        .is_none_or(str::is_empty)
    {
        identity.insert("node_id".into(), uuid::Uuid::new_v4().to_string().into());
    }
    identity.insert("display_name".into(), settings.display_name.into());
    identity.insert("advertised_url".into(), settings.advertised_url.into());
    replace(
        &identity_path,
        serde_json::to_string(&identity)?.as_bytes(),
        0o644,
    )?;

    let text = |path: &Path| path.to_string_lossy().into_owned();
    let config_text = text(&config);
    let env = [
        ("COORDINATOR_TOKEN", settings.token.clone()),
        ("GAH_ALLOW_INSECURE_HTTP", "1".to_string()),
        ("GAH_COORDINATOR_IDENTITY_PATH", text(&identity_path)),
        ("GAH_CONFIG_PATH", config_text.clone()),
        ("GAH_CONFIG", config_text),
        ("GAH_BINARY", text(&args.release.join("bin/gah"))),
        ("HOST", "0.0.0.0".to_string()),
        ("PORT", "3774".to_string()),
    ];
    let mut path_parts = vec![text(&args.release.join("bin"))];
    path_parts.extend(args.node.parent().map(text));
    path_parts.extend(
        [".local/bin", ".opencode/bin"]
            .iter()
            .map(|dir| home.join(dir))
            .filter(|dir| dir.is_dir())
            .map(|dir| text(&dir)),
    );
    let mut env_file: String = env
        .iter()
        .map(|(key, value)| format!("export {key}={}\n", shell_quote(value)))
        .collect();
    env_file.push_str(&format!(
        "export PATH={}\"$PATH\"\n",
        shell_quote(&format!("{}:", path_parts.join(":")))
    ));
    let env_path = args.root.join("worker.env");
    replace(&env_path, env_file.as_bytes(), 0o600)?;

    let header = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nsource {}\ncd {}\n",
        shell_quote(&text(&env_path)),
        shell_quote(&text(&args.release))
    );
    let node = shell_quote(&text(&args.node));
    let start = args.root.join("start.sh");
    replace(
        &start,
        format!("{header}exec {node} apps/server/dist/bin.js\n").as_bytes(),
        0o700,
    )?;
    let register = format!(
        "{header}for attempt in {{1..30}}; do\n  if curl -fsS http://127.0.0.1:3774/health >/dev/null; then break; fi\n  sleep 1\ndone\nprofiles=\"${{1:-}}\"\nif [ -z \"$profiles\" ]; then\n  profiles=\"$(\"$GAH_BINARY\" profile list --json | \"$GAH_BINARY\" installer json --each name --join ,)\"\nfi\nexec {node} apps/server/dist/registerNodeCli.js --central-url {} --self-url http://127.0.0.1:3774 --transport-mode trusted_lan --secret-ref env:COORDINATOR_TOKEN --labels windows,wsl --profiles \"$profiles\"\n",
        shell_quote(&settings.central_url)
    );
    replace(&args.root.join("register.sh"), register.as_bytes(), 0o700)?;

    let unit = format!(
        "[Unit]\nDescription=GAH headless WSL worker\nAfter=network-online.target\n\n[Service]\nExecStart=/bin/bash --login {}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(&start)
    );
    replace(
        &home.join(".config/systemd/user/gah-worker.service"),
        unit.as_bytes(),
        0o644,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[test]
    fn the_worker_gets_private_credentials_quoted_scripts_and_a_stable_identity() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home with 'quotes %");
        for sub in [".local/bin", ".opencode/bin"] {
            std::fs::create_dir_all(home.join(sub)).unwrap();
        }
        let root = home.join(".local/share/gah/worker");
        let release = root.join("release.test");
        std::fs::create_dir_all(&release).unwrap();
        let token = "secret'$dollar;still-data";
        let settings = root.join("settings.json");
        std::fs::write(
            &settings,
            serde_json::json!({
                "token": token, "central_url": "http://192.168.1.10:3773",
                "display_name": "Test Windows", "advertised_url": "http://192.168.1.11:3774"
            })
            .to_string(),
        )
        .unwrap();
        let args = Args {
            settings,
            root: root.clone(),
            release: release.clone(),
            node: PathBuf::from("/usr/bin/node"),
        };
        install(&args, &home).unwrap();
        let identity = std::fs::read_to_string(root.join("identity.json")).unwrap();
        install(&args, &home).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("identity.json")).unwrap(),
            identity,
            "node id is stable"
        );

        let env_path = root.join("worker.env");
        assert_eq!(
            std::fs::metadata(&env_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for name in ["worker.env", "start.sh", "register.sh"] {
            assert!(
                Command::new("bash")
                    .arg("-n")
                    .arg(root.join(name))
                    .status()
                    .unwrap()
                    .success(),
                "{name}"
            );
        }
        let read = |variable: &str| {
            let output = Command::new("bash")
                .args([
                    "-c",
                    &format!("source \"$1\"; printf %s \"${variable}\""),
                    "check",
                ])
                .arg(&env_path)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(read("COORDINATOR_TOKEN"), token);
        assert_eq!(
            read("GAH_CONFIG_PATH"),
            home.join(".config/gah/config.toml").to_string_lossy()
        );
        assert_eq!(
            read("GAH_BINARY"),
            release.join("bin/gah").to_string_lossy()
        );
        assert_eq!(
            read("GAH_NODE_ROLE"),
            "",
            "the role comes from config after a restart"
        );
        let path = read("PATH");
        for sub in [".local/bin", ".opencode/bin"] {
            assert!(
                path.split(':')
                    .any(|part| part == home.join(sub).to_string_lossy()),
                "{sub} in PATH"
            );
        }
        let unit =
            std::fs::read_to_string(home.join(".config/systemd/user/gah-worker.service")).unwrap();
        assert!(unit.contains("Restart=on-failure") && unit.contains("%%"));
        assert!(!unit.contains(token));
        assert!(!std::fs::read_to_string(root.join("register.sh"))
            .unwrap()
            .contains(token));
    }
}
