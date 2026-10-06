//! The shell installers, run for real against the built `gah`, with every
//! privileged or networked command stubbed. Nothing here installs a service
//! or touches the host outside a temporary directory.

use git_agent_harness::installer::plist::{parse, Plist};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GAH: &str = env!("CARGO_BIN_EXE_gah");

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script(name: &str) -> String {
    std::fs::read_to_string(repo().join("scripts").join(name)).unwrap()
}

fn executable(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn bash(args: &[&str], envs: &[(&str, &str)], clear: bool) -> Output {
    let mut command = Command::new("bash");
    command.args(args);
    if clear {
        command.env_clear();
    }
    command.envs(envs.iter().copied());
    command.output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Reads a variable the way the services do: by sourcing the file.
fn sourced(file: &Path, variable: &str) -> String {
    let output = bash(
        &[
            "-c",
            &format!("set -a; source \"$1\"; printf %s \"${variable}\""),
            "check",
            file.to_str().unwrap(),
        ],
        &[],
        false,
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn installers_need_no_python() {
    for name in [
        "install.sh",
        "install-linux.sh",
        "install-macos.sh",
        "configure-node-role.sh",
        "macos-launchd.sh",
        "install-wsl-worker.sh",
    ] {
        let source = script(name);
        assert!(!source.contains("python"), "{name} still runs Python");
        assert!(
            !source.contains("PlistBuddy"),
            "{name} still needs PlistBuddy"
        );
    }
}

#[test]
fn worker_installers_write_private_credentials_before_update() {
    let temp = tempfile::tempdir().unwrap();
    let scripts = temp.path().join("repo/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    for name in [
        "install.sh",
        "install-linux.sh",
        "install-macos.sh",
        "configure-node-role.sh",
    ] {
        std::fs::copy(repo().join("scripts").join(name), scripts.join(name)).unwrap();
    }
    let bin = temp.path().join("bin");
    executable(&bin.join("uname"), "printf '%s\\n' \"$GAH_TEST_OS\"\n");
    for name in ["sudo", "npm", "curl", "systemctl", "python3"] {
        executable(
            &bin.join(name),
            "printf 'forbidden: %s\\n' \"$0\" | tee -a \"$GAH_TEST_LOG\" >&2\nexit 99\n",
        );
    }
    // cargo runs gah: `installer` calls go to the real binary; config and
    // update are checked and recorded, not run.
    executable(
        &bin.join("cargo"),
        r#"while [ "$1" != "--" ]; do shift; done; shift
case "$1" in
  installer) exec "$GAH_TEST_BIN" "$@" ;;
esac
printf '%s\n' "$*" >> "$GAH_TEST_LOG"
case "$1" in
  config) [ "$*" = "config set --node-role worker --registry-central-url https://central.test" ] || exit 98 ;;
  update)
    [ "$2" = --repo ] && [ "$(cd "$3" && pwd -P)" = "$(pwd -P)" ] && [ "$4 $5" = "--role worker" ] || exit 97
    credential="$HOME/.config/gah/gah-loop.env"
    [ -f "$credential" ] || { echo 'credentials must exist before update starts services' >&2; exit 96; }
    ;;
  *) exit 95 ;;
esac
"#,
    );
    for os in ["Linux", "Darwin"] {
        let home = temp.path().join(os);
        std::fs::create_dir_all(&home).unwrap();
        let log = home.join("commands.log");
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let output = bash(
            &[
                "-c",
                "printf 'yes\\n' | bash \"$1\"",
                "install-test",
                scripts.join("install.sh").to_str().unwrap(),
            ],
            &[
                ("HOME", home.to_str().unwrap()),
                ("PATH", &path),
                ("GAH_TEST_OS", os),
                ("GAH_TEST_LOG", log.to_str().unwrap()),
                ("GAH_TEST_BIN", GAH),
                ("GAH_NODE_ROLE", "worker"),
                (
                    "GAH_INSTALL_CONFIRMED",
                    if os == "Linux" { "1" } else { "" },
                ),
                ("GAH_CENTRAL_URL", "https://central.test"),
                ("COORDINATOR_TOKEN", "installer-test-token"),
            ],
            true,
        );
        assert!(output.status.success(), "{os}: {}", text(&output));
        assert!(!text(&output).contains("/etc/gah"));
        let calls = std::fs::read_to_string(&log).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        assert_eq!(calls.len(), 2, "{os}: {calls:?}");
        assert!(
            calls[0].starts_with("config set")
                && calls[1].starts_with("update")
                && calls[1].ends_with("--yes"),
            "{calls:?}"
        );
        let credential = home.join(".config/gah/gah-loop.env");
        assert_eq!(
            std::fs::read_to_string(&credential).unwrap(),
            "COORDINATOR_TOKEN=\"installer-test-token\"\n"
        );
        assert_eq!(mode(&credential), 0o600);
        assert!(!home.join(".config/systemd").exists());
    }
}

#[test]
fn declining_installers_disclose_effects_before_confirmation_and_preserve_files() {
    use std::io::Write;
    use std::process::Stdio;

    for installer in ["install-linux.sh", "install-macos.sh"] {
        for role in ["worker", "central"] {
            for answer in ["n\n", ""] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let config = home.join(".config/gah");
                std::fs::create_dir_all(&config).unwrap();
                for file in [
                    "config.toml",
                    "gah-loop.env",
                    "tdai-gateway.env",
                    "server.env",
                ] {
                    std::fs::write(config.join(file), "existing settings\n").unwrap();
                }
                let memory = temp.path().join("MemoryCore");
                std::fs::create_dir_all(memory.join("src/gateway")).unwrap();
                std::fs::write(memory.join("src/gateway/server.ts"), "").unwrap();
                std::fs::write(memory.join("tdai-gateway.standalone.yaml"), "template").unwrap();
                let bin = temp.path().join("bin");
                let log = temp.path().join("commands.log");
                executable(&bin.join("uname"), "echo Darwin\n");
                for command in ["cargo", "sudo", "curl", "openssl", "systemctl", "tailscale"] {
                    executable(
                        &bin.join(command),
                        "echo forbidden >> \"$GAH_TEST_LOG\"; exit 99\n",
                    );
                }
                let mut child = Command::new("bash")
                    .arg(repo().join("scripts").join(installer))
                    .env_clear()
                    .env("HOME", &home)
                    .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                    .env("GAH_TEST_LOG", &log)
                    .env("GAH_NODE_ROLE", role)
                    .env("GAH_INSTALL_AGENT", "codex")
                    .env("COORDINATOR_TOKEN", "new-token")
                    .env("GAH_CENTRAL_URL", "https://central.test")
                    .env(
                        "GAH_GATEWAY_MODE",
                        if role == "central" { "colocated" } else { "" },
                    )
                    .env("GAH_GATEWAY_MEMORYCORE_PATH", &memory)
                    .env("GAH_GATEWAY_LLM_API_KEY", "new-key")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(answer.as_bytes())
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                assert!(
                    !output.status.success(),
                    "{installer} {role}: {}",
                    text(&output)
                );
                assert!(
                    text(&output).contains("Installation cancelled before configuration"),
                    "{}",
                    text(&output)
                );
                let stdout = String::from_utf8_lossy(&output.stdout);
                let plan = stdout.split_once("Apply these changes? [y/N]").unwrap().0;
                for effect in [
                    "$CARGO_HOME/bin",
                    "node role",
                    "gah-loop.env",
                    "accept-dns",
                    "gah-quota-refresh.service/timer",
                ] {
                    assert!(
                        plan.contains(effect),
                        "missing {effect} before approval: {stdout}"
                    );
                }
                if installer == "install-linux.sh" {
                    for effect in [
                        "gah-loop@.service",
                        "gah-watchdog.service/timer",
                        "lingering",
                        "loginctl/sudo",
                    ] {
                        assert!(plan.contains(effect), "missing {effect}: {stdout}");
                    }
                    if role == "central" {
                        // #1327: the dashboard is copied into /var/www/gah
                        // only where an earlier install created it;
                        // otherwise gah-server serves the checkout's build.
                        let legacy_root = Path::new("/var/www/gah").is_dir();
                        assert_eq!(plan.contains("/var/www/gah"), legacy_root, "{stdout}");
                        assert_eq!(
                            plan.contains("gah-server serves it"),
                            !legacy_root,
                            "{stdout}"
                        );
                        for effect in [
                            "/etc/systemd/system/gah-server.service",
                            "gah-prune.service/timer",
                            "/etc/gah/server.env",
                            "tdai-memory-gateway.service",
                            "tdai-gateway.local.yaml",
                            "tdai-gateway.env",
                        ] {
                            assert!(plan.contains(effect), "missing {effect}: {stdout}");
                        }
                    } else {
                        assert!(!plan.contains("/var/www/gah"));
                        assert!(!plan.contains("/etc/systemd/system/gah-server.service"));
                    }
                } else {
                    assert!(plan.contains("~/Applications"));
                    assert!(plan.contains("~/Library/LaunchAgents"));
                    if role == "central" {
                        assert!(plan.contains("memory-gateway LaunchAgent"));
                        assert!(plan.contains("tdai-gateway.env"));
                    }
                }
                assert!(!log.exists(), "cancellation must precede external commands");
                assert!(!memory.join("tdai-gateway.local.yaml").exists());
                assert_eq!(std::fs::read_dir(&config).unwrap().count(), 4);
                for file in [
                    "config.toml",
                    "gah-loop.env",
                    "tdai-gateway.env",
                    "server.env",
                ] {
                    assert_eq!(
                        std::fs::read_to_string(config.join(file)).unwrap(),
                        "existing settings\n"
                    );
                }
            }
        }
    }
}

#[test]
fn the_worker_role_keeps_settings_private_and_refuses_a_gateway() {
    let temp = tempfile::tempdir().unwrap();
    let cli = temp.path().join("gah");
    executable(
        &cli,
        &format!("[ \"$1\" = installer ] && exec {GAH} \"$@\"\nexit 0\n"),
    );
    let home = temp.path().join("home");
    let credentials = home.join(".config/gah/gah-loop.env");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(&credentials, "OTHER=preserved\n").unwrap();
    let configure = repo().join("scripts/configure-node-role.sh");
    let token = "secret'$dollar;\"still `data`";
    let run = |token: Option<&str>, gateway: Option<&str>| {
        let mut envs = vec![
            ("HOME", home.to_str().unwrap()),
            ("PATH", "/usr/bin:/bin"),
            ("GAH_CENTRAL_URL", "https://central.test"),
        ];
        envs.extend(token.map(|token| ("COORDINATOR_TOKEN", token)));
        envs.extend(gateway.map(|mode| ("GAH_GATEWAY_MODE", mode)));
        bash(
            &[configure.to_str().unwrap(), "worker", cli.to_str().unwrap()],
            &envs,
            true,
        )
    };
    let output = run(Some(token), None);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(sourced(&credentials, "COORDINATOR_TOKEN"), token);
    assert_eq!(sourced(&credentials, "OTHER"), "preserved");
    assert_eq!(mode(&credentials), 0o600);

    let saved = std::fs::read_to_string(&credentials).unwrap();
    assert!(run(None, None).status.success(), "a saved token is reused");
    assert_eq!(std::fs::read_to_string(&credentials).unwrap(), saved);
    assert!(!run(Some("bad\ntoken"), None).status.success());
    assert_eq!(
        std::fs::read_to_string(&credentials).unwrap(),
        saved,
        "a bad token changes nothing"
    );
    assert!(
        !run(None, Some("remote")).status.success(),
        "workers use the central relay"
    );
    std::fs::remove_file(&credentials).unwrap();
    let missing = run(None, None);
    assert!(!missing.status.success() && text(&missing).contains("requires COORDINATOR_TOKEN"));
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).unwrap() + start.len();
    &source[from..from + source[from..].find(end).unwrap()]
}

#[test]
fn the_wsl_installer_rejects_a_release_too_old_for_it() {
    let source = script("install-wsl-worker.sh");
    let check = between(
        &source,
        "install -m 755 \"$stage/gah\" \"$release_dir/bin/gah\"\n",
        "# role-cli-check:end",
    );
    assert!(
        source.find("# role-cli-check:end").unwrap() < source.find("installer wsl-worker").unwrap(),
        "the version check runs before any worker file changes"
    );
    let temp = tempfile::tempdir().unwrap();
    let cli = temp.path().join("bin/gah");
    let release = temp.path().to_str().unwrap();
    for (body, ok) in [
        ("echo 'old CLI help'\n", false),
        ("case \"$*\" in \"config set --help\"|\"status --help\") echo '--node-role --role';; *) exit 2;; esac\n", false),
        ("case \"$*\" in \"config set --help\"|\"status --help\") echo '--node-role --role';; \"installer --help\") exit 0;; *) exit 2;; esac\n", true),
    ] {
        executable(&cli, body);
        let output = bash(&["-ec", check], &[("release_dir", release)], false);
        assert_eq!(output.status.success(), ok, "{body}: {}", text(&output));
        if !ok {
            assert!(text(&output).contains("too old"));
        }
    }
}

#[test]
fn the_wsl_installer_asks_for_sudo_only_for_missing_packages() {
    let source = script("install-wsl-worker.sh");
    let packages = between(&source, "\nfi\n", "install_dir=");
    assert!(!packages.contains("python3"));
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let log = temp.path().join("sudo.log");
    executable(
        &bin.join("dpkg-query"),
        "for package; do :; done\n[ \"$package\" != \"$GAH_TEST_MISSING\" ] && printf installed\n",
    );
    executable(
        &bin.join("sudo"),
        "printf '%s\\n' \"$*\" >> \"$GAH_TEST_LOG\"\n",
    );
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let run = |missing: &str| {
        bash(
            &["-euc", packages],
            &[
                ("PATH", &path),
                ("GAH_TEST_LOG", log.to_str().unwrap()),
                ("GAH_TEST_MISSING", missing),
            ],
            false,
        )
    };
    assert!(run("").status.success());
    assert!(!log.exists(), "installed prerequisites need no sudo");
    assert!(run("xz-utils").status.success());
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "apt-get update\napt-get install -y xz-utils\n"
    );
}

#[test]
fn the_wsl_installer_reuses_a_linux_node_and_npm_and_downloads_otherwise() {
    let source = script("install-wsl-worker.sh");
    let runtime = between(&source, "# role-cli-check:end\n", "\ncd \"$release_dir\"");
    let real_node = Command::new("sh")
        .args(["-c", "command -v node"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|path| !path.is_empty())
        .expect("node is required by the WSL installer regression test");
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let native = temp.path().join("native node/bin");
    let log = temp.path().join("commands");
    // A real JS evaluator supplies Linux metadata on any host.
    executable(
        &native.join("node"),
        "exec \"$GAH_TEST_REAL_NODE\" -e 'Object.defineProperty(process, \"platform\", {value: process.env.GAH_TEST_PLATFORM}); Object.defineProperty(process, \"execPath\", {value: process.env.GAH_TEST_EXEC_PATH}); Object.defineProperty(process.versions, \"node\", {value: process.env.GAH_TEST_NODE_VERSION}); console.log(eval(process.argv[1]))' \"$2\"\n",
    );
    std::os::unix::fs::symlink(native.join("node"), bin.join("node")).unwrap_or_else(|_| {
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(native.join("node"), bin.join("node")).unwrap();
    });
    executable(&native.join("npm"), "printf 'linux-npm\\n'\n");
    executable(
        &bin.join("npm"),
        "echo 'Windows npm selected' >&2; exit 99\n",
    );
    executable(
        &bin.join("curl"),
        "echo download >> \"$GAH_TEST_LOG\"; exit 71\n",
    );
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let dir = temp.path().to_str().unwrap();
    let exec_path = native.join("node");
    let run = |extra: &str, platform: &str, version: &str| {
        bash(
            &["-euc", &format!("{runtime}{extra}")],
            &[
                ("PATH", &path),
                ("GAH_TEST_LOG", log.to_str().unwrap()),
                ("GAH_TEST_REAL_NODE", &real_node),
                ("GAH_TEST_EXEC_PATH", exec_path.to_str().unwrap()),
                ("GAH_TEST_PLATFORM", platform),
                ("GAH_TEST_NODE_VERSION", version),
                ("install_dir", dir),
                ("release_dir", dir),
            ],
            false,
        )
    };
    let reused = run("\nnpm --version\nprintf %s \"$node\"", "linux", "22.22.3");
    assert!(reused.status.success(), "{}", text(&reused));
    assert_eq!(
        String::from_utf8_lossy(&reused.stdout),
        format!("linux-npm\n{}", exec_path.display())
    );
    assert!(!log.exists(), "a compatible Linux Node and npm are reused");
    for (platform, version, has_npm) in [
        ("win32", "22.22.3", true),
        ("linux", "18.0.0", true),
        ("linux", "22.22.3", false),
    ] {
        if !has_npm {
            std::fs::remove_file(native.join("npm")).unwrap();
        }
        let output = run("", platform, version);
        assert_eq!(
            output.status.code(),
            Some(71),
            "{platform} {version} {has_npm}: {}",
            text(&output)
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "download");
        std::fs::remove_file(&log).unwrap();
    }
}

#[test]
fn the_wsl_register_script_names_every_profile_unless_told_one() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = home.join(".local/share/gah/worker");
    let release = root.join("release.test");
    std::fs::create_dir_all(release.join("apps/server/dist")).unwrap();
    let settings = root.join("settings.json");
    std::fs::write(&settings, r#"{"token":"t","central_url":"http://192.168.1.10:3773","display_name":"Win","advertised_url":"http://192.168.1.11:3774"}"#).unwrap();
    let node = temp.path().join("node");
    executable(&node, "shift; for arg; do printf '%s\\n' \"$arg\"; done\n");
    let output = Command::new(GAH)
        .args(["installer", "wsl-worker", "--settings"])
        .arg(&settings)
        .arg("--root")
        .arg(&root)
        .arg("--release")
        .arg(&release)
        .arg("--node")
        .arg(&node)
        .env("HOME", &home)
        .env("GAH_CANONICAL_CONFIG", temp.path().join("canonical.toml"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    executable(
        &release.join("bin/gah"),
        &format!("case \"$1\" in profile) printf '%s' '[{{\"name\":\"alpha\"}},{{\"name\":\"beta\"}}]';; installer) exec {GAH} \"$@\";; esac\n"),
    );
    executable(&release.join("bin/curl"), "exit 0\n");
    let register = root.join("register.sh");
    let profiles = |args: &[&str]| {
        let output = Command::new(&register)
            .args(args)
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        lines[lines.iter().position(|line| line == "--profiles").unwrap() + 1].clone()
    };
    assert_eq!(profiles(&[]), "alpha,beta");
    assert_eq!(profiles(&["only-this"]), "only-this");
}

fn agent(path: &Path) -> Plist {
    parse(&std::fs::read_to_string(path).unwrap())
        .unwrap_or_else(|| panic!("{} does not parse", path.display()))
}

fn env_of(agent: &Plist, key: &str) -> String {
    agent
        .get("EnvironmentVariables")
        .unwrap()
        .get(key)
        .and_then(Plist::as_str)
        .unwrap_or_else(|| panic!("no {key}"))
        .to_string()
}

fn arguments(agent: &Plist) -> Vec<String> {
    match agent.get("ProgramArguments") {
        Some(Plist::Array(values)) => values
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect(),
        other => panic!("ProgramArguments: {other:?}"),
    }
}

#[test]
fn macos_launch_agents_follow_role_transport_and_tunnel() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let home = root.join("home");
    let checkout = root.join("repo");
    let agents = home.join("agents");
    for file in [
        "apps/server/dist/bin.js",
        "apps/web/dist/index.html",
        "Cargo.toml",
    ] {
        std::fs::create_dir_all(checkout.join(file).parent().unwrap()).unwrap();
        std::fs::write(checkout.join(file), "").unwrap();
    }
    let memory = root.join("MemoryCore");
    for file in ["src/gateway/server.ts", "tdai-gateway.local.yaml"] {
        std::fs::create_dir_all(memory.join(file).parent().unwrap()).unwrap();
        std::fs::write(memory.join(file), "").unwrap();
    }
    std::fs::create_dir_all(home.join(".config/gah")).unwrap();
    std::fs::write(
        home.join(".config/gah/tdai-gateway.env"),
        "TDAI_GATEWAY_API_KEY=\"test\"\n",
    )
    .unwrap();
    let source = repo().join("scripts/macos-launchd.sh");
    let base: Vec<(String, String)> = [
        ("HOME", home.to_str().unwrap()),
        ("PATH", "/usr/bin:/bin"),
        ("GAH_LAUNCHD_DRY_RUN", "1"),
        ("GAH_LAUNCH_AGENTS_DIR", agents.to_str().unwrap()),
        ("GAH_LAUNCHD_UID", "501"),
        ("GAH_NODE_PATH", "/opt/homebrew/bin/node"),
        ("GAH_CLI_PATH", GAH),
        ("GAH_NPX_PATH", "/opt/homebrew/bin/npx"),
        ("GAH_GATEWAY_MEMORYCORE_PATH", memory.to_str().unwrap()),
        ("GAH_DESKTOP_SERVER_PORT", "4774"),
        ("GAH_NODE_ADVERTISED_URL", "https://mac.test.ts.net:4774"),
        (
            "GAH_TAILSCALE_PATH",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let install = |role: &str, profile: Option<&str>, env: &[(String, String)]| {
        let mut args = vec![
            source.to_str().unwrap(),
            "install",
            role,
            checkout.to_str().unwrap(),
        ];
        args.extend(profile);
        let envs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        bash(&args, &envs, true)
    };
    let without = |env: &[(String, String)], keys: &[&str]| -> Vec<(String, String)> {
        env.iter()
            .filter(|(k, _)| !keys.contains(&k.as_str()))
            .cloned()
            .collect()
    };
    let with = |env: &[(String, String)], key: &str, value: &str| -> Vec<(String, String)> {
        let mut env = without(env, &[key]);
        env.push((key.into(), value.into()));
        env
    };
    let ok = |output: Output| assert!(output.status.success(), "{}", text(&output));

    ok(install("central", None, &base));
    let central_path = agents.join("dev.git-agent-harness.server.plist");
    let central = agent(&central_path);
    assert_eq!(
        central.get("Label").and_then(Plist::as_str),
        Some("dev.git-agent-harness.server")
    );
    assert_eq!(
        arguments(&central).last().unwrap(),
        &checkout.join("apps/server/dist/bin.js").to_string_lossy()
    );
    assert_eq!(
        env_of(&central, "GAH_WEB_ROOT"),
        checkout.join("apps/web/dist").to_string_lossy()
    );
    assert_eq!(env_of(&central, "PORT"), "4774");
    assert_eq!(
        (central.get("RunAtLoad"), central.get("KeepAlive")),
        (Some(&Plist::Bool(true)), Some(&Plist::Bool(true)))
    );
    let gateway_path = agents.join("dev.git-agent-harness.memory-gateway.plist");
    let gateway = agent(&gateway_path);
    assert_eq!(
        gateway.get("WorkingDirectory").and_then(Plist::as_str),
        Some(memory.to_str().unwrap())
    );
    assert_eq!(arguments(&gateway).last().unwrap(), "/opt/homebrew/bin/npx");

    let profile = "répo<&>";
    ok(install("worker", Some(profile), &base));
    let worker_path = agents.join("dev.git-agent-harness.worker.plist");
    let worker = agent(&worker_path);
    let args = arguments(&worker);
    assert_eq!(
        args[args.len() - 2],
        checkout.join("apps/server/dist/bin.js").to_string_lossy()
    );
    assert_eq!(
        &args[..2],
        ["/bin/bash", "-c"],
        "a login shell adds seconds to every worker start"
    );
    let identity_path = home.join(".local/share/gah/worker/identity.json");
    for (key, value) in [
        ("HOST", "127.0.0.1"),
        ("PORT", "4774"),
        ("GAH_BINARY", GAH),
        ("GAH_REGISTRY_TRANSPORT_MODE", "authenticated_remote"),
        ("GAH_NODE_ADVERTISED_URL", "https://mac.test.ts.net:4774"),
        ("GAH_TAILSCALE_SERVE", "1"),
        (
            "GAH_COORDINATOR_IDENTITY_PATH",
            identity_path.to_str().unwrap(),
        ),
    ] {
        assert_eq!(env_of(&worker, key), value, "{key}");
    }
    let identity: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&identity_path).unwrap()).unwrap();
    assert_eq!(identity["advertised_url"], "https://mac.test.ts.net:4774");
    assert_eq!(mode(&identity_path), 0o600);
    assert_eq!(
        (worker.get("RunAtLoad"), worker.get("KeepAlive")),
        (Some(&Plist::Bool(false)), Some(&Plist::Bool(true)))
    );
    assert!(
        !central_path.exists(),
        "switching roles removes the old LaunchAgent"
    );
    assert!(
        !gateway_path.exists(),
        "worker mode removes the central memory gateway"
    );
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(".config/gah/desktop.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["repository_path"], checkout.to_str().unwrap());
    assert_eq!(settings["server_port"], 4774);
    assert_eq!(mode(&worker_path), 0o644);

    // An older agent without the URL: the identity and the saved port fill in.
    let Plist::Dict(mut entries) = worker else {
        unreachable!()
    };
    for (key, value) in entries.iter_mut() {
        if let (true, Plist::Dict(env)) = (key == "EnvironmentVariables", value) {
            env.retain(|(name, _)| name != "GAH_NODE_ADVERTISED_URL");
        }
    }
    std::fs::write(&worker_path, Plist::Dict(entries).to_xml()).unwrap();
    ok(install(
        "worker",
        None,
        &without(
            &base,
            &["GAH_DESKTOP_SERVER_PORT", "GAH_NODE_ADVERTISED_URL"],
        ),
    ));
    let worker = agent(&worker_path);
    assert_eq!(env_of(&worker, "PORT"), "4774");
    assert_eq!(
        env_of(&worker, "GAH_REGISTRY_TRANSPORT_MODE"),
        "authenticated_remote"
    );

    let tailscale = root.join("tailscale");
    executable(&tailscale, "printf '%s\\n' '{\"Self\":{\"DNSName\":\"mac.test.ts.net.\",\"TailscaleIPs\":[\"100.64.0.42\",\"fd7a:115c:a1e0::1\"]}}'\n");
    std::fs::remove_file(&worker_path).unwrap();
    std::fs::remove_file(&identity_path).unwrap();
    let default_transport = with(
        &without(&base, &["GAH_NODE_ADVERTISED_URL"]),
        "GAH_TAILSCALE_PATH",
        tailscale.to_str().unwrap(),
    );
    ok(install("worker", Some(profile), &default_transport));
    let worker = agent(&worker_path);
    let identity: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&identity_path).unwrap()).unwrap();
    assert_eq!(identity["advertised_url"], "http://100.64.0.42:4774");
    assert_eq!(env_of(&worker, "HOST"), "100.64.0.42");
    assert_eq!(
        env_of(&worker, "GAH_REGISTRY_TRANSPORT_MODE"),
        "trusted_lan"
    );
    assert_eq!(env_of(&worker, "GAH_TAILSCALE_SERVE"), "0");

    let tunnel_env = with(
        &with(
            &default_transport,
            "GAH_NODE_SSH_TARGET",
            "khing@central.test",
        ),
        "GAH_NODE_SSH_REMOTE_PORT",
        "48774",
    );
    ok(install("worker", Some(profile), &tunnel_env));
    let worker = agent(&worker_path);
    let tunnel_path = agents.join("dev.git-agent-harness.worker-tunnel.plist");
    let identity: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&identity_path).unwrap()).unwrap();
    assert_eq!(identity["advertised_url"], "http://127.0.0.1:48774");
    assert_eq!(env_of(&worker, "HOST"), "127.0.0.1");
    assert_eq!(env_of(&worker, "GAH_REGISTRY_TRANSPORT_MODE"), "loopback");
    assert_eq!(env_of(&worker, "GAH_ALLOW_INSECURE_HTTP"), "0");
    let tunnel = arguments(&agent(&tunnel_path));
    assert_eq!(
        &tunnel[tunnel.len() - 3..],
        ["-R", "127.0.0.1:48774:127.0.0.1:4774", "khing@central.test"]
    );

    let preserved = without(
        &tunnel_env,
        &[
            "GAH_DESKTOP_SERVER_PORT",
            "GAH_NODE_SSH_TARGET",
            "GAH_NODE_SSH_REMOTE_PORT",
        ],
    );
    ok(install("worker", Some(profile), &preserved));
    let worker = agent(&worker_path);
    assert_eq!(env_of(&worker, "PORT"), "4774");
    assert_eq!(env_of(&worker, "GAH_REGISTRY_TRANSPORT_MODE"), "loopback");
    assert!(
        tunnel_path.exists(),
        "an update keeps the managed SSH tunnel"
    );

    ok(install(
        "worker",
        Some(profile),
        &with(
            &preserved,
            "GAH_NODE_ADVERTISED_URL",
            "https://mac.test.ts.net:4774",
        ),
    ));
    assert_eq!(
        env_of(&agent(&worker_path), "GAH_REGISTRY_TRANSPORT_MODE"),
        "authenticated_remote"
    );
    assert!(
        !tunnel_path.exists(),
        "an explicit URL change replaces the tunnel transport"
    );

    let invalid = install(
        "central",
        None,
        &with(&base, "GAH_DESKTOP_SERVER_PORT", "80"),
    );
    assert!(
        !invalid.status.success() && text(&invalid).contains("between 1024 and 65535"),
        "{}",
        text(&invalid)
    );
    ok(install("central", None, &base));
    assert!(
        !tunnel_path.exists(),
        "central mode removes the worker tunnel"
    );
}

#[test]
fn the_desktop_app_install_replaces_the_old_app_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("repo");
    std::fs::create_dir_all(checkout.join("apps/desktop")).unwrap();
    let built = checkout.join("apps/cargo-target/release/bundle/macos/GAH.app");
    std::fs::create_dir_all(&built).unwrap();
    std::fs::write(built.join("version.txt"), "new").unwrap();
    let apps = temp.path().join("Applications");
    let legacy = apps.join("GAH Worker.app");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("version.txt"), "old").unwrap();
    let home = temp.path().join("home");
    let output = bash(
        &[
            repo()
                .join("scripts/install-macos-desktop.sh")
                .to_str()
                .unwrap(),
            checkout.to_str().unwrap(),
        ],
        &[
            ("HOME", home.to_str().unwrap()),
            ("PATH", "/usr/bin:/bin"),
            ("CARGO_TARGET_DIR", "../cargo-target"),
            ("GAH_DESKTOP_APP_DIR", apps.to_str().unwrap()),
            ("GAH_DESKTOP_SKIP_BUILD", "1"),
        ],
        true,
    );
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        std::fs::read_to_string(apps.join("GAH.app/version.txt")).unwrap(),
        "new"
    );
    assert!(
        !legacy.exists(),
        "the old product name must not leave a second app"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&apps)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".gah-desktop.") || name == ".GAH.previous.app")
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging and backups are removed: {leftovers:?}"
    );
}

#[test]
fn colocated_installers_preserve_credentials_without_a_generation_key() {
    for platform in ["linux", "macos"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        executable(&bin.join("gah"), "exec \"$GAH_TEST_BIN\" \"$@\"\n");
        executable(
            &bin.join("cargo"),
            "while [ \"$1\" != -- ]; do shift; done; shift\nexec \"$GAH_TEST_BIN\" \"$@\"\n",
        );
        let file = home.join(".config/gah/tdai-gateway.env");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // Credential canaries are fixtures, never supplied to a provider.
        let before = "TDAI_GATEWAY_API_KEY=\"access-canary\"\nTDAI_LLM_API_KEY=\"generation-canary\"\nTDAI_EMBEDDING_API_KEY=\"embedding-canary\"\n";
        std::fs::write(&file, before).unwrap();
        let source = script(&format!("install-{platform}.sh"));
        let block = if platform == "linux" {
            let start = source.find("# gateway-env-setup:start\n").unwrap();
            let end = source[start..].find("# gateway-env-setup:end").unwrap() + start;
            format!(
                "{}\n{}",
                &source[source.find("upsert_env_line() {").unwrap()
                    ..source.find("# Used by both").unwrap()],
                &source[start..end]
            )
        } else {
            let start = source.find("# gateway-env-setup:start\n").unwrap();
            let end = source[start..].find("# gateway-env-setup:end").unwrap() + start;
            format!(
                "gah_cli=(cargo run --locked -q --bin gah --)\n{}",
                &source[start..end]
            )
        };
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let run = |given: &[(&str, &str)]| {
            let mut envs = vec![
                ("HOME", home.to_str().unwrap()),
                ("PATH", path.as_str()),
                ("GAH_TEST_BIN", GAH),
            ];
            envs.extend(given);
            let output = bash(&["-euc", &block], &envs, true);
            assert!(output.status.success(), "{platform}: {}", text(&output));
            assert_eq!(mode(&file), 0o600);
        };
        run(&[]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);

        // The previous Linux installer wrote bare, unquoted values.
        let bare = "TDAI_GATEWAY_API_KEY=access-canary\nTDAI_LLM_API_KEY=generation-canary\n";
        std::fs::write(&file, bare).unwrap();
        run(&[]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), bare);

        // Given keys replace only their own lines.
        std::fs::write(&file, before).unwrap();
        run(&[
            ("GAH_GATEWAY_API_KEY", "new-access"),
            ("GAH_GATEWAY_LLM_API_KEY", "new-generation"),
        ]);
        assert_eq!(sourced(&file, "TDAI_GATEWAY_API_KEY"), "new-access");
        assert_eq!(sourced(&file, "TDAI_LLM_API_KEY"), "new-generation");
        assert_eq!(sourced(&file, "TDAI_EMBEDDING_API_KEY"), "embedding-canary");

        std::fs::remove_file(&file).unwrap();
        run(&[]);
        assert!(!sourced(&file, "TDAI_GATEWAY_API_KEY").is_empty());
        assert!(sourced(&file, "TDAI_LLM_API_KEY").is_empty());
    }
}
#[test]
fn colocated_installers_mutate_provider_yaml_safely() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let memory_core = home.join("MemoryCore");
    std::fs::create_dir_all(&memory_core).unwrap();
    std::fs::write(
        memory_core.join("package.json"),
        "{\"name\":\"MemoryCore\"}",
    )
    .unwrap();
    // The installer resolves `yaml` from the MemoryCore checkout; pin the
    // version the supported MemoryCore fork declares (^2.8.3).
    let npm_output = Command::new("npm")
        .args([
            "install",
            "--no-audit",
            "--no-fund",
            "--no-save",
            "yaml@2.8.3",
        ])
        .current_dir(&memory_core)
        .output()
        .unwrap();
    assert!(
        npm_output.status.success(),
        "npm install yaml failed: {}",
        text(&npm_output)
    );
    let memory_core_path = memory_core.to_string_lossy().into_owned();
    let path_env = format!(
        "/usr/bin:/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );

    for platform in ["linux", "macos"] {
        let source = script(&format!("install-{platform}.sh"));
        let start = source.find("# gateway-yaml-mutation:start\n").unwrap();
        let end = source[start..].find("# gateway-yaml-mutation:end").unwrap() + start;
        let block = &source[start..end];
        let config_path = home.join("tdai-gateway.local.yaml");

        let run = |given: &[(&str, &str)]| {
            std::fs::write(&config_path, "{\"llm\":{}, \"memory\":{}}").unwrap();
            let mut envs = vec![
                ("HOME", home.to_str().unwrap()),
                ("PATH", path_env.as_str()),
                ("GAH_GATEWAY_MEMORYCORE_PATH", memory_core_path.as_str()),
                ("gateway_local_config", config_path.to_str().unwrap()),
                ("gateway_config", config_path.to_str().unwrap()),
            ];
            envs.extend(given);
            let output = bash(&["-euc", block], &envs, true);
            if !output.status.success() {
                return Err(text(&output));
            }
            let print_json = r#"
const yaml = require('yaml');
const fs = require('fs');
console.log(JSON.stringify(yaml.parse(fs.readFileSync(process.argv[1], 'utf8'))));
"#;
            let check_output = Command::new("node")
                .arg("-e")
                .arg(print_json)
                .arg(&config_path)
                .current_dir(&memory_core)
                .output()
                .unwrap();
            assert!(check_output.status.success(), "failed to parse yaml");
            Ok(serde_json::from_slice::<serde_json::Value>(&check_output.stdout).unwrap())
        };

        // The supported MemoryCore contract disables a remote embedding
        // provider unless apiKey, baseUrl, model, and dimensions are all set.
        let complete = |json: &serde_json::Value| {
            let embedding = &json["memory"]["embedding"];
            for field in ["apiKey", "baseUrl", "model"] {
                assert!(
                    embedding[field].as_str().is_some_and(|v| !v.is_empty()),
                    "{platform}: embedding.{field} missing: {json}"
                );
            }
            assert!(embedding["dimensions"].as_u64().is_some_and(|d| d > 0));
        };

        // Ollama defaults use its OpenAI-compatible /v1 API and a
        // non-secret placeholder key, because Ollama ignores credentials.
        let ollama = run(&[("GAH_GATEWAY_PROVIDER", "ollama")]).unwrap();
        complete(&ollama);
        assert_eq!(ollama["llm"]["baseUrl"], "http://127.0.0.1:11434/v1");
        assert_eq!(ollama["llm"]["model"], "llama3");
        assert_eq!(ollama["llm"]["apiKey"], "ollama");
        assert_eq!(ollama["memory"]["embedding"]["provider"], "ollama");
        assert_eq!(
            ollama["memory"]["embedding"]["baseUrl"],
            "http://127.0.0.1:11434/v1"
        );
        assert_eq!(ollama["memory"]["embedding"]["model"], "nomic-embed-text");
        assert_eq!(ollama["memory"]["embedding"]["dimensions"], 768);
        assert_eq!(ollama["memory"]["embedding"]["sendDimensions"], false);
        assert_eq!(ollama["memory"]["embedding"]["apiKey"], "ollama");

        // Explicit values, a custom model with explicit dimensions, and a
        // given embedding key, which the gateway reads from its env file.
        let custom = run(&[
            ("GAH_GATEWAY_PROVIDER", "ollama"),
            ("GAH_GATEWAY_ENDPOINT", "http://test:11434/v1"),
            ("GAH_GATEWAY_LLM_MODEL", "my-llama"),
            ("GAH_GATEWAY_EMBEDDING_MODEL", "my-embed"),
            ("GAH_GATEWAY_EMBEDDING_DIMENSIONS", "512"),
            ("GAH_GATEWAY_EMBEDDING_API_KEY", "embedding-canary"),
        ])
        .unwrap();
        complete(&custom);
        assert_eq!(custom["llm"]["baseUrl"], "http://test:11434/v1");
        assert_eq!(custom["llm"]["model"], "my-llama");
        assert_eq!(custom["memory"]["embedding"]["model"], "my-embed");
        assert_eq!(custom["memory"]["embedding"]["dimensions"], 512);
        assert_eq!(custom["memory"]["embedding"]["sendDimensions"], false);
        assert_eq!(
            custom["memory"]["embedding"]["apiKey"],
            "${TDAI_EMBEDDING_API_KEY}"
        );
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            !written.contains("embedding-canary"),
            "{platform}: {written}"
        );

        // An unknown model without dimensions fails instead of guessing.
        let unknown = run(&[
            ("GAH_GATEWAY_PROVIDER", "ollama"),
            ("GAH_GATEWAY_EMBEDDING_MODEL", "my-embed"),
        ])
        .unwrap_err();
        assert!(
            unknown.contains("GAH_GATEWAY_EMBEDDING_DIMENSIONS"),
            "{platform}: {unknown}"
        );
        let invalid = run(&[("GAH_GATEWAY_PROVIDER", "constructor")]).unwrap_err();
        assert!(
            invalid.contains("openai or ollama"),
            "{platform}: {invalid}"
        );

        // OpenAI defaults reference the stored credentials, never literals, unless an API key was explicitly given.
        let openai = run(&[("GAH_GATEWAY_PROVIDER", "openai")]).unwrap();
        complete(&openai);
        assert_eq!(openai["llm"]["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(openai["llm"]["model"], "gpt-4o");
        assert_eq!(openai["llm"]["apiKey"], "");
        assert_eq!(openai["memory"]["embedding"]["provider"], "openai");
        assert_eq!(
            openai["memory"]["embedding"]["baseUrl"],
            "https://api.openai.com/v1"
        );
        assert_eq!(
            openai["memory"]["embedding"]["model"],
            "text-embedding-3-small"
        );
        assert_eq!(openai["memory"]["embedding"]["dimensions"], 1536);
        assert_eq!(openai["memory"]["embedding"]["sendDimensions"], true);
        assert_eq!(
            openai["memory"]["embedding"]["apiKey"],
            "${TDAI_EMBEDDING_API_KEY}"
        );

        // Shell metacharacters in an endpoint reach the YAML verbatim.
        let injection = run(&[
            ("GAH_GATEWAY_PROVIDER", "ollama"),
            ("GAH_GATEWAY_ENDPOINT", "https://example.test/v1?a=1&b=2"),
        ])
        .unwrap();
        assert_eq!(
            injection["llm"]["baseUrl"],
            "https://example.test/v1?a=1&b=2"
        );
    }
}
