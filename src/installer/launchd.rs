//! The files `scripts/macos-launchd.sh install` writes: the central or worker
//! LaunchAgent, the worker's identity and optional SSH tunnel, the memory
//! gateway agent, and the desktop app's settings. launchctl stays in the
//! script; this decides and writes what it loads.

use super::files::replace;
use super::plist::Plist;
use anyhow::{bail, Context, Result};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    #[arg(long, value_parser = ["central", "worker"])]
    pub role: String,
    /// The GAH checkout, already resolved.
    #[arg(long)]
    pub repo: PathBuf,
    #[arg(long, default_value = "")]
    pub profile: String,
    #[arg(long)]
    pub label: String,
    /// Where the role's LaunchAgent goes; its siblings go beside it.
    #[arg(long)]
    pub plist: PathBuf,
    #[arg(long)]
    pub node: String,
    #[arg(long, default_value = "")]
    pub gah: String,
    #[arg(long)]
    pub port: String,
    #[arg(long, default_value = "")]
    pub advertised_url: String,
    #[arg(long, default_value = "")]
    pub tailscale: String,
    #[arg(long, default_value = "")]
    pub transport_mode: String,
    #[arg(long, default_value = "")]
    pub tunnel_target: String,
    #[arg(long, default_value = "")]
    pub tunnel_remote_port: String,
    #[arg(long, default_value = "")]
    pub npx: String,
    /// A MemoryCore checkout to run the memory gateway from (central only).
    /// Omit to keep the one saved in desktop.json.
    #[arg(long, default_value = "")]
    pub memorycore: String,
    /// PATH for the agents. Defaults to this process's PATH.
    #[arg(long)]
    pub path: Option<String>,
}

const TUNNEL_LABEL: &str = "dev.git-agent-harness.worker-tunnel";
const GATEWAY_LABEL: &str = "dev.git-agent-harness.memory-gateway";

fn port_number(value: &str, name: &str) -> Result<u16> {
    match value.parse::<u16>() {
        Ok(port) if port >= 1024 && value.chars().all(|c| c.is_ascii_digit()) => Ok(port),
        _ => bail!("{name} must be between 1024 and 65535"),
    }
}

struct WorkerTransport {
    advertised_url: String,
    mode: &'static str,
    host: String,
    serve: bool,
}

fn worker_transport(args: &Args, port: u16) -> Result<WorkerTransport> {
    let url = url::Url::parse(&args.advertised_url).map_err(|_| {
        anyhow::anyhow!(
            "GAH_NODE_ADVERTISED_URL must be an HTTP(S) origin without credentials or a path"
        )
    })?;
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        bail!("GAH_NODE_ADVERTISED_URL must be an HTTP(S) origin without credentials or a path");
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_string();
    let loopback = host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    let mode = match args.transport_mode.as_str() {
        "" if loopback => "loopback",
        "" if scheme == "https" => "authenticated_remote",
        "" => "trusted_lan",
        "loopback" => "loopback",
        "authenticated_remote" => "authenticated_remote",
        "trusted_lan" => "trusted_lan",
        _ => {
            bail!("GAH_NODE_TRANSPORT_MODE must be loopback, authenticated_remote, or trusted_lan")
        }
    };
    if mode != "loopback" && url.port_or_known_default() != Some(port) {
        bail!("a non-loopback GAH_NODE_ADVERTISED_URL must use GAH_DESKTOP_SERVER_PORT");
    }
    let (host, serve) = match mode {
        "loopback" => {
            if scheme != "http" || !loopback {
                bail!("loopback transport requires an http://127.0.0.1 advertised URL");
            }
            ("127.0.0.1".to_string(), false)
        }
        "authenticated_remote" => {
            if scheme != "https" {
                bail!("authenticated_remote transport requires an HTTPS advertised URL");
            }
            if args.tailscale.is_empty() {
                bail!("Tailscale is required for an HTTPS macOS worker");
            }
            ("127.0.0.1".to_string(), true)
        }
        _ => {
            if scheme != "http" {
                bail!("trusted_lan transport requires an HTTP advertised URL");
            }
            let ip: Ipv4Addr = host.parse().map_err(|_| {
                anyhow::anyhow!("an HTTP GAH_NODE_ADVERTISED_URL must use this Mac IPv4 address")
            })?;
            (ip.to_string(), false)
        }
    };
    Ok(WorkerTransport {
        advertised_url: args.advertised_url.clone(),
        mode,
        host,
        serve,
    })
}

pub fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates
    // within it on success.
    let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(0);
    if ok && end > 0 {
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    } else {
        "GAH worker".to_string()
    }
}

fn read_json(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn write_json(path: &Path, value: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    replace(
        path,
        format!("{}\n", serde_json::to_string_pretty(value)?).as_bytes(),
        0o600,
    )
}

/// Keeps a worker's node id across reinstalls; names it after the machine
/// unless it already has a name of its own.
fn write_identity(home: &Path, repo: &Path, advertised_url: &str) -> Result<PathBuf> {
    let path = home.join(".local/share/gah/worker/identity.json");
    let source = if path.exists() {
        path.clone()
    } else {
        repo.join("config/coordinator-identity.json")
    };
    let mut identity = read_json(&source);
    if identity
        .get("node_id")
        .and_then(|v| v.as_str())
        .is_none_or(str::is_empty)
    {
        identity.insert("node_id".into(), uuid::Uuid::new_v4().to_string().into());
    }
    let named = identity
        .get("display_name")
        .and_then(|v| v.as_str())
        .is_some_and(|name| !name.is_empty() && name != "GAH Coordinator");
    if !named {
        identity.insert("display_name".into(), hostname().into());
    }
    identity.insert("advertised_url".into(), advertised_url.into());
    write_json(&path, &identity)?;
    Ok(path)
}

fn env(entries: Vec<(&str, String)>) -> Plist {
    Plist::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), Plist::String(v)))
            .collect(),
    )
}

pub fn install(args: &Args, home: &Path) -> Result<()> {
    let port = port_number(&args.port, "GAH_DESKTOP_SERVER_PORT")?;
    let repo_text = args.repo.to_string_lossy();
    for value in [repo_text.as_ref(), &args.profile, &args.advertised_url] {
        if value.chars().any(char::is_control) {
            bail!("launchd values must not contain control characters");
        }
    }
    let path = args
        .path
        .clone()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    let worker = args.role == "worker";
    let transport = if worker {
        Some(worker_transport(args, port)?)
    } else {
        None
    };
    let worker_identity = home.join(".local/share/gah/worker/identity.json");
    if let Some(transport) = &transport {
        write_identity(home, &args.repo, &transport.advertised_url)?;
    }

    let settings_path = home.join(".config/gah/desktop.json");
    let mut settings = read_json(&settings_path);
    let gateway_repo = if args.memorycore.is_empty() {
        settings
            .get("gateway_repository_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    } else {
        std::fs::canonicalize(&args.memorycore)
            .with_context(|| format!("resolving {}", args.memorycore))?
            .to_string_lossy()
            .into_owned()
    };
    settings.insert("repository_path".into(), repo_text.as_ref().into());
    settings.insert("server_port".into(), port.into());
    if !args.memorycore.is_empty() {
        settings.insert(
            "gateway_repository_path".into(),
            gateway_repo.clone().into(),
        );
    }
    write_json(&settings_path, &settings)?;

    let log = |name: &str| {
        home.join(format!(".local/state/gah/{name}.log"))
            .to_string_lossy()
            .into_owned()
    };
    let home_text = home.to_string_lossy().into_owned();
    let config = home
        .join(".config/gah/config.toml")
        .to_string_lossy()
        .into_owned();
    let server = args.repo.join("apps/server/dist/bin.js");
    let server_text = server.to_string_lossy().into_owned();
    let mut agent = vec![
        ("Label", Plist::String(args.label.clone())),
        ("WorkingDirectory", Plist::String(repo_text.to_string())),
        ("ProcessType", Plist::String("Background".into())),
        ("StandardOutPath", Plist::String(log(&args.role))),
        ("StandardErrorPath", Plist::String(log(&args.role))),
    ];
    match &transport {
        None => {
            if !server.is_file() || !args.repo.join("apps/web/dist/index.html").is_file() {
                bail!("central build is missing; run gah update --role central first");
            }
            let source = "set -a; [ ! -f \"$HOME/.config/gah/server.env\" ] || . \"$HOME/.config/gah/server.env\"; [ ! -f \"$HOME/.config/gah/tdai-gateway.env\" ] || . \"$HOME/.config/gah/tdai-gateway.env\"; set +a; exec \"$0\" \"$1\"";
            agent.extend([
                (
                    "ProgramArguments",
                    Plist::strings(["/bin/bash", "-lc", source, &args.node, &server_text]),
                ),
                (
                    "EnvironmentVariables",
                    env(vec![
                        ("HOME", home_text.clone()),
                        ("PATH", path.clone()),
                        ("NODE_ENV", "production".into()),
                        ("HOST", "127.0.0.1".into()),
                        ("PORT", port.to_string()),
                        ("GAH_CONFIG_PATH", config.clone()),
                        (
                            "GAH_WEB_ROOT",
                            args.repo
                                .join("apps/web/dist")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ("GAH_ENABLE_ADMIN_UPDATE", "1".into()),
                    ]),
                ),
                ("RunAtLoad", Plist::Bool(true)),
            ]);
        }
        Some(transport) => {
            if !server.is_file() {
                bail!("worker build is missing; run gah update --role worker first");
            }
            let identity = worker_identity.to_string_lossy().into_owned();
            let source = "set -a; [ ! -f \"$HOME/.config/gah/gah-loop.env\" ] || . \"$HOME/.config/gah/gah-loop.env\"; set +a; export GAH_COORDINATOR_IDENTITY_PATH=\"$2\"; exec \"$0\" \"$1\"";
            agent.extend([
                (
                    "ProgramArguments",
                    Plist::strings([
                        "/bin/bash",
                        "-c",
                        source,
                        &args.node,
                        &server_text,
                        &identity,
                    ]),
                ),
                (
                    "EnvironmentVariables",
                    env(vec![
                        ("HOME", home_text.clone()),
                        ("PATH", path.clone()),
                        ("NODE_ENV", "production".into()),
                        ("HOST", transport.host.clone()),
                        ("PORT", port.to_string()),
                        ("GAH_CONFIG", config.clone()),
                        ("GAH_CONFIG_PATH", config.clone()),
                        ("GAH_BINARY", args.gah.clone()),
                        ("GAH_COORDINATOR_IDENTITY_PATH", identity),
                        ("GAH_NODE_ADVERTISED_URL", transport.advertised_url.clone()),
                        (
                            "GAH_ALLOW_INSECURE_HTTP",
                            if transport.mode == "trusted_lan" {
                                "1"
                            } else {
                                "0"
                            }
                            .into(),
                        ),
                        ("GAH_REGISTRY_TRANSPORT_MODE", transport.mode.into()),
                        ("GAH_TAILSCALE_CLI", args.tailscale.clone()),
                        (
                            "GAH_TAILSCALE_SERVE",
                            if transport.serve { "1" } else { "0" }.into(),
                        ),
                        (
                            "XDG_STATE_HOME",
                            home.join(".local/state").to_string_lossy().into_owned(),
                        ),
                        (
                            "TMPDIR",
                            home.join(".cache/gah/tmp").to_string_lossy().into_owned(),
                        ),
                    ]),
                ),
                ("RunAtLoad", Plist::Bool(false)),
            ]);
        }
    }
    agent.extend([
        ("KeepAlive", Plist::Bool(true)),
        ("ThrottleInterval", Plist::Integer(5)),
    ]);
    // launchd ignores key order; this keeps the files diffable.
    let agent = Plist::dict(agent);

    let agents = args.plist.parent().unwrap_or(Path::new("."));
    let tunnel_plist = agents.join(format!("{TUNNEL_LABEL}.plist"));
    match &transport {
        Some(transport) if !args.tunnel_target.is_empty() => {
            if args.tunnel_target.chars().any(char::is_whitespace)
                || args.tunnel_target.starts_with('-')
            {
                bail!("GAH_NODE_SSH_TARGET must be one SSH destination without options");
            }
            let remote_port = port_number(&args.tunnel_remote_port, "GAH_NODE_SSH_REMOTE_PORT")?;
            let advertised_port = url::Url::parse(&transport.advertised_url)
                .ok()
                .and_then(|url| url.port());
            if transport.mode != "loopback" || advertised_port != Some(remote_port) {
                bail!("an SSH tunnel requires loopback transport and an advertised URL on its remote port");
            }
            let tunnel = Plist::dict(vec![
                ("Label", Plist::String(TUNNEL_LABEL.into())),
                (
                    "ProgramArguments",
                    Plist::strings([
                        "/usr/bin/ssh",
                        "-NT",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ExitOnForwardFailure=yes",
                        "-o",
                        "ServerAliveInterval=30",
                        "-o",
                        "ServerAliveCountMax=3",
                        "-R",
                        &format!("127.0.0.1:{remote_port}:127.0.0.1:{port}"),
                        &args.tunnel_target,
                    ]),
                ),
                (
                    "EnvironmentVariables",
                    env(vec![("HOME", home_text.clone()), ("PATH", path.clone())]),
                ),
                ("RunAtLoad", Plist::Bool(false)),
                ("KeepAlive", Plist::Bool(true)),
                ("ThrottleInterval", Plist::Integer(5)),
                ("ProcessType", Plist::String("Background".into())),
                ("StandardOutPath", Plist::String(log("worker-tunnel"))),
                ("StandardErrorPath", Plist::String(log("worker-tunnel"))),
            ]);
            replace(&tunnel_plist, tunnel.to_xml().as_bytes(), 0o644)?;
        }
        // A loopback worker without a new target keeps the tunnel it has.
        Some(transport) if transport.mode == "loopback" => {}
        _ => remove(&tunnel_plist)?,
    }

    replace(&args.plist, agent.to_xml().as_bytes(), 0o644)?;

    let gateway_plist = agents.join(format!("{GATEWAY_LABEL}.plist"));
    if worker {
        remove(&gateway_plist)?;
    } else if !gateway_repo.is_empty() {
        let gateway_repo =
            std::fs::canonicalize(&gateway_repo).unwrap_or_else(|_| PathBuf::from(&gateway_repo));
        let gateway_config = gateway_repo.join("tdai-gateway.local.yaml");
        if !gateway_repo.join("src/gateway/server.ts").is_file()
            || !gateway_config.is_file()
            || !home.join(".config/gah/tdai-gateway.env").is_file()
            || args.npx.is_empty()
        {
            bail!("the saved TDAI gateway is incomplete; rerun install-macos.sh with GAH_GATEWAY_MODE=colocated");
        }
        let gateway = Plist::dict(vec![
            ("Label", Plist::String(GATEWAY_LABEL.into())),
            ("ProgramArguments", Plist::strings([
                "/bin/bash", "-lc",
                "set -a; . \"$HOME/.config/gah/tdai-gateway.env\"; set +a; exec \"$0\" tsx src/gateway/server.ts",
                &args.npx,
            ])),
            ("WorkingDirectory", Plist::String(gateway_repo.to_string_lossy().into_owned())),
            ("EnvironmentVariables", env(vec![
                ("HOME", home_text.clone()), ("PATH", path.clone()),
                ("TDAI_GATEWAY_CONFIG", gateway_config.to_string_lossy().into_owned()),
            ])),
            ("RunAtLoad", Plist::Bool(true)),
            ("KeepAlive", Plist::Bool(true)),
            ("ThrottleInterval", Plist::Integer(5)),
            ("ProcessType", Plist::String("Background".into())),
            ("StandardOutPath", Plist::String(log("memory-gateway"))),
            ("StandardErrorPath", Plist::String(log("memory-gateway"))),
        ]);
        replace(&gateway_plist, gateway.to_xml().as_bytes(), 0o644)?;
    }
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}
