//! The one list of what each GAH feature needs from a machine (#new-user
//! setup). The wizard, `gah setup --check --json`, and the README's
//! requirements table all read it, so they cannot disagree.
//!
//! A requirement is checked the moment the list is built; re-checking means
//! building the list again after an install.

use super::host::{self, Host, Os, PackageManager};
use crate::auth_health::{self, AuthState};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Run agents on a repository from this machine.
    Core,
    /// The central server: dashboard, phone access, chats, fleet.
    Dashboard,
    /// Run work for a central node on another machine.
    Worker,
    /// Shared memory across chats and dispatches (the memory gateway).
    Memory,
}

impl Feature {
    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "Run agents on your repositories",
            Self::Dashboard => "Dashboard and phone control (this machine is the central node)",
            Self::Worker => "Worker for another central node",
            Self::Memory => "Shared memory across chats and runs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Central,
    Standalone,
    Worker,
    CliOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Opencode];

    pub fn command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code (Anthropic)",
            Self::Codex => "Codex (OpenAI)",
            Self::Opencode => "opencode (many providers, including GitHub Copilot)",
        }
    }

    fn npm_package(self) -> &'static str {
        match self {
            Self::Claude => "@anthropic-ai/claude-code",
            Self::Codex => "@openai/codex",
            Self::Opencode => "opencode-ai",
        }
    }

    fn login(self) -> &'static str {
        match self {
            Self::Claude => "claude auth login",
            Self::Codex => "codex login",
            Self::Opencode => "opencode auth login",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Github,
    Gitlab,
}

impl Provider {
    pub fn cli(self) -> &'static str {
        match self {
            Self::Github => "gh",
            Self::Gitlab => "glab",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    Off,
    /// Run the gateway on this machine (needs a MemoryCore checkout and an LLM key).
    Colocated,
    /// Use a gateway already running elsewhere.
    Remote,
}

/// What the user asked for. Everything else follows from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub role: Role,
    pub agent: Agent,
    pub provider: Provider,
    pub memory: MemoryMode,
}

impl Selection {
    pub fn features(&self) -> Vec<Feature> {
        let mut features = vec![Feature::Core];
        match self.role {
            Role::Central | Role::Standalone => features.push(Feature::Dashboard),
            Role::Worker => features.push(Feature::Worker),
            Role::CliOnly => {}
        }
        // A worker's memory goes through its central node's relay.
        if self.memory != MemoryMode::Off && matches!(self.role, Role::Central | Role::Standalone) {
            features.push(Feature::Memory);
        }
        features
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Ok {
        found: Option<String>,
    },
    Missing,
    Outdated {
        found: String,
    },
    /// No saved login: the provider CLI says there is no account.
    NotLoggedIn {
        reason: String,
    },
    /// A saved login exists, but the provider rejected its credential.
    CredentialsRejected {
        reason: String,
    },
    /// The provider CLI answered, but its status could not be recognized.
    StatusUnknown {
        reason: String,
    },
    /// The status check itself failed or did not finish, so the login state
    /// is unknown; the user may well be logged in (#1324).
    StatusFailed {
        reason: String,
    },
    /// Cannot be provided on this machine (e.g. no systemd).
    Unsupported {
        reason: String,
    },
}

impl Status {
    pub fn is_ok(&self) -> bool {
        matches!(self, Status::Ok { .. })
    }

    /// The login check did not settle the login either way (#1324), so the
    /// fix is to check again, not to log in again.
    pub fn is_unresolved(&self) -> bool {
        matches!(
            self,
            Status::StatusUnknown { .. } | Status::StatusFailed { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Install,
    Login,
}

/// How setup can provide a missing requirement: one shell command run with
/// the terminal attached, after the user agrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Action {
    pub kind: ActionKind,
    pub command: String,
    /// Asks for an administrator password (system package manager).
    pub sudo: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Requirement {
    pub id: &'static str,
    pub label: String,
    /// Why this machine needs it, in the user's terms.
    pub why: &'static str,
    pub feature: Feature,
    /// Recommended, not required: setup continues without it.
    pub optional: bool,
    pub status: Status,
    pub action: Option<Action>,
    /// Where to read how to get it when setup cannot run the action.
    pub help: Option<&'static str>,
}

impl Requirement {
    pub fn blocking(&self) -> bool {
        // An unconfirmed login may well be valid (#1324): setup warns about
        // it instead of refusing to build.
        !self.optional && !self.status.is_ok() && !self.status.is_unresolved()
    }

    /// The requirement that must be in place before this one can be
    /// provided: npm packages need Node, and a login needs its program.
    pub fn needs(&self) -> Option<&'static str> {
        match self.id {
            "agent" => Some("node"),
            "agent_login" => Some("agent"),
            "provider_login" => Some("provider_cli"),
            _ => None,
        }
    }
}

fn package(host: &dyn Host, names: &[(PackageManager, &str)]) -> Option<Action> {
    let manager = host.package_manager()?;
    let name = names.iter().find(|(candidate, _)| *candidate == manager)?.1;
    let (command, sudo) = match manager {
        PackageManager::Brew => (format!("brew install {name}"), false),
        PackageManager::Apt => (format!("sudo apt-get install -y {name}"), true),
        PackageManager::Dnf => (format!("sudo dnf install -y {name}"), true),
        PackageManager::Pacman => (format!("sudo pacman -S --noconfirm {name}"), true),
    };
    Some(Action {
        kind: ActionKind::Install,
        command,
        sudo,
    })
}

fn same_everywhere(name: &str) -> Vec<(PackageManager, &str)> {
    [
        PackageManager::Brew,
        PackageManager::Apt,
        PackageManager::Dnf,
        PackageManager::Pacman,
    ]
    .into_iter()
    .map(|manager| (manager, name))
    .collect()
}

fn command_status(
    host: &dyn Host,
    program: &str,
    args: &[&str],
    minimum: Option<(u32, u32)>,
) -> Status {
    let Some(probe) = host.probe(program, args) else {
        return Status::Missing;
    };
    let text = format!("{}{}", probe.stdout, probe.stderr);
    let found = text
        .lines()
        .next()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty());
    match (minimum, host::version(&text)) {
        (Some(minimum), Some(version)) if version < minimum => Status::Outdated {
            found: found.unwrap_or_default(),
        },
        _ => Status::Ok { found },
    }
}

/// A login check that did not finish says nothing about the login (#1324).
/// The program already answered `--version`, so `None` means the check
/// hung past the probe budget or could not start.
fn unfinished() -> Status {
    Status::StatusFailed {
        reason: auth_health::timed_out().detail.unwrap_or_default(),
    }
}

fn login_status(host: &dyn Host, program: &str, args: &[&str]) -> Status {
    // gh validates its token over the network, which can outlast the
    // 20-second probe budget on a slow link (#1324), so retry once.
    let Some(probe) = host
        .probe(program, args)
        .or_else(|| host.probe(program, args))
    else {
        return unfinished();
    };
    let (stdout, stderr) = (probe.stdout.as_bytes(), probe.stderr.as_bytes());
    let health = match program {
        "claude" => auth_health::classify_claude_status(probe.success, stdout),
        "gh" => auth_health::classify_gh_status("github.com", probe.success, stdout, stderr),
        _ => auth_health::classify_status_output(probe.success, stdout, stderr),
    };
    // Every classifier explains a state that is not Ok.
    let reason = health.detail.unwrap_or_default();
    match health.state {
        AuthState::Ok => Status::Ok { found: None },
        AuthState::Missing => Status::NotLoggedIn { reason },
        AuthState::Expired => Status::CredentialsRejected { reason },
        AuthState::Unknown => Status::StatusUnknown { reason },
        AuthState::Error => Status::StatusFailed { reason },
    }
}

const NODE_INSTALL: &str = "curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash && . \"$HOME/.nvm/nvm.sh\" && nvm install 22";
const RUST_INSTALL: &str =
    "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y";

/// Everything the selection needs, checked on this host, in the order setup
/// should provide it (a program before its login; Node before npm packages).
pub fn requirements(selection: &Selection, host: &dyn Host) -> Vec<Requirement> {
    let features = selection.features();
    let wants = |feature: Feature| features.contains(&feature);
    let server = wants(Feature::Dashboard) || wants(Feature::Worker);
    let os = host.os();
    let mut list = Vec::new();

    list.push(Requirement {
        id: "git",
        label: "git".into(),
        why: "Checks out your repositories and the work branches agents create.",
        feature: Feature::Core,
        optional: false,
        status: command_status(host, "git", &["--version"], None),
        action: if os == Os::Macos {
            Some(Action {
                kind: ActionKind::Install,
                command: "xcode-select --install".into(),
                sudo: false,
            })
        } else {
            package(host, &same_everywhere("git"))
        },
        help: Some("https://git-scm.com/downloads"),
    });
    list.push(Requirement {
        id: "rust",
        label: "Rust toolchain (cargo)".into(),
        why: "Builds gah itself, and rebuilds it on every `gah update`.",
        feature: Feature::Core,
        optional: false,
        status: command_status(host, "cargo", &["--version"], None),
        action: Some(Action {
            kind: ActionKind::Install,
            command: RUST_INSTALL.into(),
            sudo: false,
        }),
        help: Some("https://rustup.rs"),
    });
    let agent = selection.agent;
    let min_node = match agent {
        Agent::Claude => 22,
        _ => 20,
    };
    list.push(Requirement {
        id: "node",
        label: format!("Node.js {min_node} or newer"),
        why: "Installs your agent CLI through npm, and runs the dashboard server and memory gateway.",
        feature: Feature::Core,
        optional: false,
        status: command_status(host, "node", &["--version"], Some((min_node, 0))),
        action: Some(Action { kind: ActionKind::Install, command: NODE_INSTALL.into(), sudo: false }),
        help: Some("https://nodejs.org/en/download"),
    });
    let agent_status = command_status(host, agent.command(), &["--version"], None);
    let agent_installed = agent_status.is_ok();
    list.push(Requirement {
        id: "agent",
        label: agent.label().into(),
        why: "The coding agent GAH runs for you.",
        feature: Feature::Core,
        optional: false,
        status: agent_status,
        action: Some(Action {
            kind: ActionKind::Install,
            command: format!("npm install -g {}", agent.npm_package()),
            sudo: false,
        }),
        help: None,
    });
    list.push(Requirement {
        id: "agent_login",
        label: format!("{} login", agent.command()),
        why: "The agent needs its own account to do any work.",
        feature: Feature::Core,
        optional: false,
        status: if !agent_installed {
            Status::NotLoggedIn {
                reason: "agent CLI is missing".to_string(),
            }
        } else {
            match agent {
                Agent::Claude => login_status(host, "claude", &["auth", "status", "--json"]),
                Agent::Codex => login_status(host, "codex", &["login", "status"]),
                Agent::Opencode => match host.probe("opencode", &["auth", "list"]) {
                    Some(probe) if probe.stdout.contains('●') => Status::Ok { found: None },
                    Some(_) => Status::NotLoggedIn {
                        reason: "Not logged in.".to_string(),
                    },
                    None => unfinished(),
                },
            }
        },
        action: Some(Action {
            kind: ActionKind::Login,
            command: agent.login().into(),
            sudo: false,
        }),
        help: None,
    });

    let cli = selection.provider.cli();
    let provider_status = command_status(host, cli, &["--version"], None);
    let provider_installed = provider_status.is_ok();
    list.push(Requirement {
        id: "provider_cli",
        label: format!(
            "{cli} ({} CLI)",
            if cli == "gh" { "GitHub" } else { "GitLab" }
        ),
        why: "Reads issues and opens pull requests on your repositories.",
        feature: Feature::Core,
        optional: false,
        status: provider_status,
        action: package(
            host,
            &if cli == "gh" {
                vec![
                    (PackageManager::Brew, "gh"),
                    (PackageManager::Apt, "gh"),
                    (PackageManager::Dnf, "gh"),
                    (PackageManager::Pacman, "github-cli"),
                ]
            } else {
                vec![
                    (PackageManager::Brew, "glab"),
                    (PackageManager::Apt, "glab"),
                    (PackageManager::Dnf, "glab"),
                    (PackageManager::Pacman, "glab"),
                ]
            },
        ),
        help: Some(if cli == "gh" {
            "https://cli.github.com"
        } else {
            "https://gitlab.com/gitlab-org/cli#installation"
        }),
    });
    list.push(Requirement {
        id: "provider_login",
        label: format!("{cli} login"),
        why: "Lets GAH read issues and push branches as you.",
        feature: Feature::Core,
        optional: false,
        status: if provider_installed {
            login_status(host, cli, &["auth", "status"])
        } else {
            Status::NotLoggedIn {
                reason: "provider CLI is missing".to_string(),
            }
        },
        action: Some(Action {
            kind: ActionKind::Login,
            command: format!("{cli} auth login"),
            sudo: false,
        }),
        help: None,
    });

    if server {
        let feature = if wants(Feature::Dashboard) {
            Feature::Dashboard
        } else {
            Feature::Worker
        };
        list.push(Requirement {
            id: "curl",
            label: "curl".into(),
            why: "The installer uses it to check that services came up.",
            feature,
            optional: false,
            status: command_status(host, "curl", &["--version"], None),
            action: package(host, &same_everywhere("curl")),
            help: None,
        });
        list.push(Requirement {
            id: "service_manager",
            label: if os == Os::Macos {
                "launchd".into()
            } else {
                "systemd".into()
            },
            why: "Keeps the GAH server running and restarts it after a reboot.",
            feature,
            optional: false,
            status: match os {
                Os::Macos => Status::Ok { found: None },
                Os::Linux => match host.probe("systemctl", &["--version"]) {
                    Some(probe) if probe.success => Status::Ok {
                        found: probe.stdout.lines().next().map(str::to_string),
                    },
                    _ => Status::Unsupported {
                        reason: "This Linux machine has no systemd; GAH's service needs it.".into(),
                    },
                },
                Os::Other => Status::Unsupported {
                    reason: "Use Linux or macOS for the server; Windows runs a worker in WSL."
                        .into(),
                },
            },
            action: None,
            help: None,
        });
        if os == Os::Linux {
            list.push(user_lingering(host, feature));
        }
        // Tailscale is a networked-node concern: worker<->central transport,
        // or reaching a central dashboard from other machines. A standalone
        // host is loopback-only by definition (#1318): setup must not install,
        // configure, prompt for, or require it, so the requirement is absent
        // rather than optional.
        if selection.role != Role::Standalone {
            list.push(Requirement {
                id: "tailscale",
                label: "Tailscale".into(),
                why: if feature == Feature::Worker {
                    "The simplest private network between this worker and its central node."
                } else {
                    "Reach the dashboard from your phone and other machines over HTTPS, privately."
                },
                feature,
                optional: true,
                status: command_status(host, "tailscale", &["version"], None),
                action: match os {
                    Os::Macos => package(host, &[(PackageManager::Brew, "--cask tailscale")]),
                    Os::Linux => Some(Action {
                        kind: ActionKind::Install,
                        command: "curl -fsSL https://tailscale.com/install.sh | sh".into(),
                        sudo: true,
                    }),
                    Os::Other => None,
                },
                help: Some("https://tailscale.com/download"),
            });
        }
    }

    if wants(Feature::Memory) && selection.memory == MemoryMode::Colocated {
        list.push(Requirement {
            id: "openssl",
            label: "openssl".into(),
            why: "Generates the memory gateway's access key.",
            feature: Feature::Memory,
            optional: false,
            status: command_status(host, "openssl", &["version"], None),
            action: package(host, &same_everywhere("openssl")),
            help: None,
        });
    }
    list
}

/// Set while systemd is PID 1 (`sd_booted()`). `systemctl --version` alone
/// also succeeds in WSL or containers where systemd is not running.
pub(crate) const SYSTEMD_RUNNING: &str = "/run/systemd/system";

/// logind's record of a lingering account. Read as a file so an account with
/// no session still reads correctly.
pub(crate) fn linger_path(user: &str) -> PathBuf {
    Path::new("/var/lib/systemd/linger").join(user)
}

/// Issue #1347: without lingering, the user units stop at reboot until
/// someone logs in. Optional, like `gah update`'s attempt: a host without
/// sudo still finishes setup, and the gap stays visible here. No systemd or
/// no usable account name means Unsupported with no action, never a command
/// that cannot work.
fn user_lingering(host: &dyn Host, feature: Feature) -> Requirement {
    let user = if host.exists(Path::new(SYSTEMD_RUNNING)) {
        host.probe("id", &["-un"])
            .filter(|p| p.success)
            .map(|p| p.stdout.trim().to_string())
            .filter(|user| {
                !user.is_empty()
                    && user
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            })
            // Same charset units::render accepts for the service user.
            .ok_or("id -un named no account GAH can run its units as.")
    } else {
        Err("Lingering needs systemd running.")
    };
    Requirement {
        id: "user_lingering",
        label: "user lingering".into(),
        why: "Keeps the user's systemd manager and timers running after they log out.",
        feature,
        optional: true,
        status: match &user {
            Err(reason) => Status::Unsupported {
                reason: (*reason).into(),
            },
            Ok(user) if host.exists(&linger_path(user)) => Status::Ok { found: None },
            Ok(_) => Status::Missing,
        },
        action: user.ok().map(|user| Action {
            kind: ActionKind::Install,
            command: format!("sudo loginctl enable-linger {user}"),
            sudo: true,
        }),
        help: None,
    }
}

/// A machine with nothing installed, for describing requirements without
/// checking them.
struct Blank;

impl Host for Blank {
    fn os(&self) -> Os {
        Os::Linux
    }
    fn package_manager(&self) -> Option<PackageManager> {
        None
    }
    fn probe(&self, _program: &str, _args: &[&str]) -> Option<host::Probe> {
        None
    }
    fn exists(&self, _path: &std::path::Path) -> bool {
        false
    }
    fn env(&self, _key: &str) -> Option<String> {
        None
    }
}

/// The requirements table in docs/GETTING_STARTED.md, rendered from this
/// module so the docs cannot drift from what setup checks.
pub fn markdown_table() -> String {
    let selections = [
        (Role::CliOnly, MemoryMode::Off),
        (Role::Central, MemoryMode::Colocated),
        (Role::Worker, MemoryMode::Off),
    ];
    let mut rows: Vec<(Requirement, Vec<&'static str>)> = Vec::new();
    for (role, memory) in selections {
        let selection = Selection {
            role,
            agent: Agent::Claude,
            provider: Provider::Github,
            memory,
        };
        for requirement in requirements(&selection, &Blank) {
            let feature = match requirement.feature {
                Feature::Core => "Everything",
                Feature::Dashboard => "Dashboard",
                Feature::Worker => "Worker",
                Feature::Memory => "Shared memory",
            };
            match rows.iter_mut().find(|(row, _)| row.id == requirement.id) {
                Some((_, features)) if !features.contains(&feature) => features.push(feature),
                Some(_) => {}
                None => rows.push((requirement, vec![feature])),
            }
        }
    }
    let mut table = String::from("| Requirement | Needed for | Why |\n| --- | --- | --- |\n");
    for (requirement, features) in rows {
        let label = match requirement.id {
            "agent" => "Your coding agent (Claude Code, Codex, or opencode)".to_string(),
            "agent_login" => "Your coding agent's login".to_string(),
            "provider_cli" => "`gh` (GitHub) or `glab` (GitLab)".to_string(),
            "provider_login" => "`gh` or `glab` login".to_string(),
            "service_manager" => "systemd (Linux) or launchd (macOS)".to_string(),
            _ => requirement.label.clone(),
        };
        let needed = if requirement.optional {
            format!("{} (recommended)", features.join(", "))
        } else {
            features.join(", ")
        };
        table.push_str(&format!("| {label} | {needed} | {} |\n", requirement.why));
    }
    table
}

/// The machine-readable check the desktop installers read (`gah setup --check --json`).
#[derive(Debug, Serialize)]
pub struct Report {
    /// Local application prerequisites are independent of factory module activation.
    pub application_ready: bool,
    pub factory_enabled: bool,
    pub factory_ready: bool,
    pub os: Os,
    pub package_manager: Option<PackageManager>,
    pub selection: Selection,
    pub features: Vec<Feature>,
    pub requirements: Vec<Requirement>,
    /// Every required item is present; optional ones may still be missing.
    pub ready: bool,
}

pub fn report(selection: Selection, host: &dyn Host) -> Report {
    let requirements = requirements(&selection, host);
    Report {
        application_ready: !requirements.iter().any(Requirement::blocking),
        factory_enabled: false,
        factory_ready: false,
        os: host.os(),
        package_manager: host.package_manager(),
        features: selection.features(),
        ready: !requirements.iter().any(Requirement::blocking),
        selection,
        requirements,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::setup::host::Probe;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// A machine described by the programs it has and what they print.
    pub(crate) struct FakeHost {
        pub os: Os,
        pub manager: Option<PackageManager>,
        pub programs: HashMap<String, Probe>,
        pub paths: Vec<PathBuf>,
    }

    impl FakeHost {
        pub(crate) fn new(os: Os, manager: Option<PackageManager>) -> Self {
            Self {
                os,
                manager,
                programs: HashMap::new(),
                paths: Vec::new(),
            }
        }
        pub(crate) fn with(self, invocation: &str, success: bool, stdout: &str) -> Self {
            self.with_streams(invocation, success, stdout, "")
        }
        pub(crate) fn with_streams(
            mut self,
            invocation: &str,
            success: bool,
            stdout: &str,
            stderr: &str,
        ) -> Self {
            self.programs.insert(
                invocation.into(),
                Probe {
                    success,
                    stdout: stdout.into(),
                    stderr: stderr.into(),
                },
            );
            self
        }
        pub(crate) fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
            self.paths.push(path.into());
            self
        }
    }

    impl Host for FakeHost {
        fn os(&self) -> Os {
            self.os
        }
        fn package_manager(&self) -> Option<PackageManager> {
            self.manager
        }
        fn probe(&self, program: &str, args: &[&str]) -> Option<Probe> {
            self.programs
                .get(&format!("{program} {}", args.join(" ")))
                .cloned()
        }
        fn exists(&self, path: &Path) -> bool {
            self.paths.iter().any(|known| known == path)
        }
        fn env(&self, _key: &str) -> Option<String> {
            None
        }
    }

    fn selection(role: Role) -> Selection {
        Selection {
            role,
            agent: Agent::Claude,
            provider: Provider::Github,
            memory: MemoryMode::Off,
        }
    }

    fn ids(list: &[Requirement]) -> Vec<&'static str> {
        list.iter().map(|requirement| requirement.id).collect()
    }

    #[test]
    fn a_cli_only_machine_needs_no_server_dependencies() {
        let list = requirements(
            &selection(Role::CliOnly),
            &FakeHost::new(Os::Linux, Some(PackageManager::Apt)),
        );
        assert_eq!(
            ids(&list),
            [
                "git",
                "rust",
                "node",
                "agent",
                "agent_login",
                "provider_cli",
                "provider_login"
            ]
        );
    }

    #[test]
    fn the_dashboard_adds_its_service_needs_and_tailscale_is_only_recommended() {
        let list = requirements(
            &selection(Role::Central),
            &FakeHost::new(Os::Linux, Some(PackageManager::Apt)),
        );
        assert!(ids(&list).ends_with(&["curl", "service_manager", "user_lingering", "tailscale"]));
        let tailscale = list.iter().find(|r| r.id == "tailscale").unwrap();
        assert!(tailscale.optional && !tailscale.blocking());
        let systemd = list.iter().find(|r| r.id == "service_manager").unwrap();
        assert!(
            matches!(systemd.status, Status::Unsupported { .. }),
            "no systemctl means no service"
        );
    }

    /// Issue #1347: lingering never blocks setup, and only offers a command
    /// when systemd is running and the account has a shell-safe name.
    #[test]
    fn user_lingering_needs_systemd_running_and_never_blocks() {
        let host =
            || FakeHost::new(Os::Linux, Some(PackageManager::Apt)).with("id -un", true, "testuser");
        let lingering = |host: FakeHost| {
            requirements(&selection(Role::Worker), &host)
                .into_iter()
                .find(|r| r.id == "user_lingering")
                .unwrap()
        };

        let missing = lingering(host().with_path(SYSTEMD_RUNNING));
        assert_eq!(missing.status, Status::Missing);
        assert!(!missing.blocking());
        assert_eq!(
            missing.action.unwrap().command,
            "sudo loginctl enable-linger testuser"
        );

        let on = lingering(
            host()
                .with_path(SYSTEMD_RUNNING)
                .with_path(linger_path("testuser")),
        );
        assert!(on.status.is_ok());

        for unsupported in [
            host(),
            FakeHost::new(Os::Linux, None)
                .with_path(SYSTEMD_RUNNING)
                .with("id -un", true, "DOMAIN\\khing"),
        ] {
            let lingering = lingering(unsupported);
            assert!(matches!(lingering.status, Status::Unsupported { .. }));
            assert!(lingering.action.is_none() && !lingering.blocking());
        }
    }

    #[test]
    fn standalone_setup_never_prompts_for_tailscale() {
        let list = requirements(
            &selection(Role::Standalone),
            &FakeHost::new(Os::Linux, Some(PackageManager::Apt)),
        );
        assert!(
            !ids(&list).contains(&"tailscale"),
            "standalone is loopback-only: setup must not install, configure, prompt for, or require Tailscale (#1318)"
        );
        assert!(ids(&list).ends_with(&["curl", "service_manager", "user_lingering"]));
    }

    #[test]
    fn memory_is_central_only_and_colocated_adds_openssl() {
        let mut central = selection(Role::Central);
        central.memory = MemoryMode::Colocated;
        assert!(central.features().contains(&Feature::Memory));
        assert!(ids(&requirements(&central, &FakeHost::new(Os::Linux, None))).contains(&"openssl"));
        let mut worker = selection(Role::Worker);
        worker.memory = MemoryMode::Colocated;
        assert!(
            !worker.features().contains(&Feature::Memory),
            "a worker uses its central node's memory relay"
        );
    }

    #[test]
    fn statuses_versions_and_logins_are_read_from_the_machine() {
        let host = FakeHost::new(Os::Linux, Some(PackageManager::Apt))
            .with("git --version", true, "git version 2.43.0")
            .with("node --version", true, "v18.19.0")
            .with("claude --version", true, "2.1.0 (Claude Code)")
            .with("claude auth status --json", false, r#"{"loggedIn":false}"#)
            .with("gh --version", true, "gh version 2.60.0")
            .with(
                "gh auth status",
                true,
                "✓ Logged in to github.com account octo (keyring)",
            );
        let list = requirements(&selection(Role::CliOnly), &host);
        let status = |id: &str| list.iter().find(|r| r.id == id).unwrap().status.clone();
        assert!(status("git").is_ok());
        assert_eq!(status("rust"), Status::Missing);
        assert_eq!(
            status("node"),
            Status::Outdated {
                found: "v18.19.0".into()
            }
        );
        assert_eq!(
            status("agent_login"),
            Status::NotLoggedIn {
                reason: "Not logged in.".into()
            }
        );
        assert!(status("provider_login").is_ok());
    }

    /// Issue #1324: setup detection with the tester's gh (2.45.0) and its
    /// account-state shapes, captured from the real binary. Every state
    /// stays distinct instead of collapsing into "not logged in", and a
    /// probe that cannot run is never reported as a missing login.
    #[test]
    fn gh_245_login_states_stay_distinct() {
        let with_gh = |success: bool, stdout: &str, stderr: &str| {
            FakeHost::new(Os::Linux, Some(PackageManager::Apt))
                .with("gh --version", true, "gh version 2.45.0")
                .with_streams("gh auth status", success, stdout, stderr)
        };
        let provider_login = |host: &FakeHost| {
            requirements(&selection(Role::CliOnly), host)
                .iter()
                .find(|requirement| requirement.id == "provider_login")
                .unwrap()
                .status
                .clone()
        };
        assert_eq!(
            provider_login(&with_gh(
                true,
                "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n  - Git operations protocol: https\n  - Token: gho_************\n  - Token scopes: 'gist', 'read:org', 'repo'\n",
                ""
            )),
            Status::Ok { found: None }
        );
        assert_eq!(
            provider_login(&with_gh(
                true,
                "github.com\n  X Failed to log in to github.com account octo (keyring)\n  - Active account: true\n  - The token in keyring is invalid.\n  - To re-authenticate, run: gh auth login -h github.com\n",
                ""
            )),
            Status::CredentialsRejected {
                reason: "The saved login has expired.".into()
            }
        );
        assert_eq!(
            provider_login(&with_gh(
                true,
                "github.com\n  X Timeout trying to log in to github.com account octo (keyring)\n  - Active account: true\n",
                ""
            )),
            Status::StatusUnknown {
                reason: "The login status was not recognized.".into()
            }
        );
        assert_eq!(
            provider_login(&with_gh(
                false,
                "",
                "You are not logged into any GitHub hosts. To log in, run: gh auth login\n"
            )),
            Status::NotLoggedIn {
                reason: "Not logged in.".into()
            }
        );
        assert_eq!(
            provider_login(&with_gh(false, "", "")),
            Status::StatusFailed {
                reason: "The login status command failed.".into()
            }
        );
        // A fresh login listed next to an old account whose token is dead.
        assert_eq!(
            provider_login(&with_gh(
                true,
                "github.com\n  X Failed to log in to github.com account old (keyring)\n  - Active account: false\n  - The token in keyring is invalid.\n\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n",
                ""
            )),
            Status::Ok { found: None }
        );
        let hung = FakeHost::new(Os::Linux, Some(PackageManager::Apt)).with(
            "gh --version",
            true,
            "gh version 2.45.0",
        );
        assert_eq!(
            provider_login(&hung),
            Status::StatusFailed {
                reason: "The login check did not finish.".into()
            }
        );
    }

    /// Issue #1324: the states must stay distinct in the machine-readable
    /// report the desktop app reads, not only in memory.
    #[test]
    fn login_states_serialize_distinctly() {
        let states = [
            (
                Status::NotLoggedIn {
                    reason: "Not logged in.".into(),
                },
                "not_logged_in",
            ),
            (
                Status::CredentialsRejected {
                    reason: "The saved login has expired.".into(),
                },
                "credentials_rejected",
            ),
            (
                Status::StatusUnknown {
                    reason: "The login status was not recognized.".into(),
                },
                "status_unknown",
            ),
            (
                Status::StatusFailed {
                    reason: "The login status command failed.".into(),
                },
                "status_failed",
            ),
        ];
        for (status, state) in states {
            assert_eq!(serde_json::to_value(&status).unwrap()["state"], state);
        }
    }

    #[test]
    fn install_actions_match_the_package_manager_and_flag_sudo() {
        let apt = requirements(
            &selection(Role::Central),
            &FakeHost::new(Os::Linux, Some(PackageManager::Apt)),
        );
        let gh = apt
            .iter()
            .find(|r| r.id == "provider_cli")
            .unwrap()
            .action
            .clone()
            .unwrap();
        assert_eq!(
            (gh.command.as_str(), gh.sudo),
            ("sudo apt-get install -y gh", true)
        );
        let mac = requirements(
            &selection(Role::Central),
            &FakeHost::new(Os::Macos, Some(PackageManager::Brew)),
        );
        let gh = mac
            .iter()
            .find(|r| r.id == "provider_cli")
            .unwrap()
            .action
            .clone()
            .unwrap();
        assert_eq!((gh.command.as_str(), gh.sudo), ("brew install gh", false));
        let bare = requirements(&selection(Role::CliOnly), &FakeHost::new(Os::Linux, None));
        let gh = bare.iter().find(|r| r.id == "provider_cli").unwrap();
        assert!(
            gh.action.is_none() && gh.help.is_some(),
            "no package manager: point to the download page"
        );
    }

    #[test]
    fn getting_started_lists_what_setup_checks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/GETTING_STARTED.md");
        let doc = std::fs::read_to_string(&path).unwrap();
        let (start, end) = ("<!-- requirements:start -->\n", "<!-- requirements:end -->");
        let from = doc.find(start).expect("requirements:start marker") + start.len();
        let to = doc.find(end).expect("requirements:end marker");
        let table = markdown_table();
        if std::env::var("GAH_UPDATE_DOCS").as_deref() == Ok("1") {
            std::fs::write(&path, format!("{}{}{}", &doc[..from], table, &doc[to..])).unwrap();
            return;
        }
        assert_eq!(
            &doc[from..to],
            table,
            "docs/GETTING_STARTED.md is stale: run GAH_UPDATE_DOCS=1 cargo test --lib setup::"
        );
    }

    #[test]
    fn the_report_is_ready_only_when_nothing_required_is_missing() {
        let host = FakeHost::new(Os::Macos, Some(PackageManager::Brew));
        assert!(!report(selection(Role::CliOnly), &host).ready);
        let json = serde_json::to_value(report(selection(Role::CliOnly), &host)).unwrap();
        assert_eq!(json["requirements"][0]["status"]["state"], "missing");
        assert_eq!(json["selection"]["role"], "cli_only");
    }

    #[test]
    fn node_20_is_rejected_because_claude_acp_requires_22() {
        let host = FakeHost::new(Os::Linux, None).with("node --version", true, "v20.20.1");
        let list = requirements(&selection(Role::CliOnly), &host);
        let status = list.iter().find(|r| r.id == "node").unwrap().status.clone();
        assert_eq!(
            status,
            Status::Outdated {
                found: "v20.20.1".into()
            }
        );
    }
}
