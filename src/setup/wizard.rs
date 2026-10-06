//! `gah setup`: the guided install. It asks what the machine is for, checks
//! every prerequisite before anything is built, offers to provide what is
//! missing (never without a yes), installs the service through
//! `scripts/install.sh`, adds the first project, and ends with what was set
//! up and how to add the rest later.

use super::host::{self, Host, Os};
use super::project;
use super::requirements::{
    self, Action, ActionKind, Agent, MemoryMode, Provider, Requirement, Role, Selection, Status,
};
use crate::init::InitArgs;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub trait Prompter {
    fn say(&mut self, line: &str);
    fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize>;
    fn confirm(&mut self, question: &str, default: bool) -> Result<bool>;
    fn text(&mut self, question: &str, default: Option<&str>) -> Result<String>;
    fn secret(&mut self, question: &str) -> Result<String>;
}

/// Everything setup does to the machine beyond reading it.
pub trait Effects {
    /// Runs one shell command with the terminal attached.
    fn run(&mut self, command: &str, cwd: Option<&Path>, env: &[(&str, String)]) -> bool;
    /// The HTTP status of one request, or `None` when nothing answered.
    fn http(
        &mut self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> Option<u16>;
    fn profile_exists(&mut self, profile: &str) -> bool;
    fn add_profile(&mut self, args: InitArgs) -> Result<()>;
}

/// Choices given on the command line; anything unset is asked (or, with
/// `--yes`, takes its default). Secrets only ever come from the environment.
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub role: Option<Role>,
    pub agent: Option<Agent>,
    pub provider: Option<Provider>,
    pub memory: Option<MemoryMode>,
    pub project: Option<PathBuf>,
    pub central_url: Option<String>,
    pub gateway_url: Option<String>,
    pub memorycore: Option<PathBuf>,
    pub source: PathBuf,
    pub yes: bool,
}

pub struct Setup<'a> {
    pub host: &'a dyn Host,
    pub prompter: &'a mut dyn Prompter,
    pub effects: &'a mut dyn Effects,
    pub options: Options,
}

/// Values the service installer receives through its environment.
#[derive(Default)]
struct InstallEnv(Vec<(&'static str, String)>);

impl<'a> Setup<'a> {
    /// How to run setup again right now: `gah setup` once gah is installed,
    /// else the binary the paste script just built.
    fn again(&self) -> String {
        if self.host.probe("gah", &["--version"]).is_some() {
            "gah setup".into()
        } else {
            format!(
                "{} setup",
                self.options.source.join("target/release/gah").display()
            )
        }
    }

    fn ask_choice(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        if self.options.yes {
            Ok(default)
        } else {
            self.prompter.choose(question, options, default)
        }
    }

    fn ask_confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        if self.options.yes {
            Ok(default)
        } else {
            self.prompter.confirm(question, default)
        }
    }

    fn secret_from(&mut self, variable: &str, question: &str) -> Result<String> {
        if let Some(value) = self.host.env(variable) {
            return Ok(value);
        }
        if self.options.yes {
            bail!("{variable} is required; set it in the environment for an unattended setup.");
        }
        let value = self.prompter.secret(question)?;
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            bail!("That value is empty or contains control characters.");
        }
        Ok(value.trim().to_string())
    }

    fn installed(&self, program: &str) -> bool {
        self.host.probe(program, &["--version"]).is_some()
    }

    pub fn run(mut self) -> Result<()> {
        let say = |this: &mut Self, line: &str| this.prompter.say(line);
        say(
            &mut self,
            "GAH setup. Nothing is installed without asking you first.",
        );
        say(&mut self, "");

        // 1. What is this machine for?
        // #1318: node networking is opt-in. On Linux -- where the loopback-only
        // standalone role is supported -- accepting the default sets up the
        // local machine; a networked role (central or worker) is an explicit
        // choice. macOS has no standalone support yet, so its default stays
        // the central role and standalone remains an explicit choice.
        let role = match self.options.role {
            Some(role) => role,
            None => {
                let (options, roles) = if self.host.os() == Os::Linux {
                    (
                        [
                            "A local dashboard and worker on this machine (standalone)".to_string(),
                            "My main machine: dashboard, chats, and phone control (central node)"
                                .to_string(),
                            "A worker that runs jobs for a central node I already have".to_string(),
                            "Just the command line, no server".to_string(),
                        ],
                        [Role::Standalone, Role::Central, Role::Worker, Role::CliOnly],
                    )
                } else {
                    (
                        [
                            "My main machine: dashboard, chats, and phone control (central node)"
                                .to_string(),
                            "A worker that runs jobs for a central node I already have".to_string(),
                            "Just the command line, no server".to_string(),
                            "A local dashboard and worker on this machine (standalone)".to_string(),
                        ],
                        [Role::Central, Role::Worker, Role::CliOnly, Role::Standalone],
                    )
                };
                roles[self.ask_choice("What is this machine for?", &options, 0)?]
            }
        };
        if role == Role::Standalone && self.host.os() != Os::Linux {
            bail!("Standalone setup requires Linux.");
        }

        // 2. A repository to work on names its provider.
        // A worker's projects come from the central dashboard (Chat → import).
        let checkout = if role == Role::Worker {
            None
        } else {
            self.first_project()?
        };
        let provider = match (
            self.options.provider,
            checkout.as_ref().and_then(|checkout| checkout.provider),
        ) {
            (Some(provider), _) | (None, Some(provider)) => provider,
            (None, None) => {
                let options = [
                    "GitHub".to_string(),
                    "GitLab (gitlab.com or self-hosted)".to_string(),
                ];
                [Provider::Github, Provider::Gitlab]
                    [self.ask_choice("Where are your repositories hosted?", &options, 0)?]
            }
        };

        // 3. Which agent? Default to one that is already installed.
        let agent = match self.options.agent {
            Some(agent) => agent,
            None => {
                let default = Agent::ALL
                    .iter()
                    .position(|agent| self.installed(agent.command()))
                    .unwrap_or(0);
                let options: Vec<String> = Agent::ALL
                    .iter()
                    .map(|agent| {
                        format!(
                            "{}{}",
                            agent.label(),
                            if self.installed(agent.command()) {
                                " (installed)"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect();
                Agent::ALL[self.ask_choice(
                    "Which coding agent will you use? You can add others later.",
                    &options,
                    default,
                )?]
            }
        };

        // 4. Role-specific settings, checked before anything is built.
        let mut env = InstallEnv::default();
        let mut memory = MemoryMode::Off;
        match role {
            Role::Worker => self.worker_settings(&mut env)?,
            Role::Central | Role::Standalone => memory = self.memory_settings(&mut env)?,
            Role::CliOnly => {}
        }
        let selection = Selection {
            role,
            agent,
            provider,
            memory,
        };

        // 5. Every prerequisite, with an offer for each missing one.
        self.provide_requirements(&selection)?;

        // 6. The service (or just the CLI), then the first project.
        self.install(&selection, env)?;
        if let Some(checkout) = checkout {
            let profile = project::profile_name(&checkout.repo);
            if self.effects.profile_exists(&profile) {
                say(
                    &mut self,
                    &format!("✓ Project {profile} is already configured."),
                );
            } else {
                self.effects
                    .add_profile(project::init_args(&checkout, provider, &profile))?;
                say(
                    &mut self,
                    &format!("✓ Added project {profile} ({}).", checkout.repo),
                );
            }
        }

        self.summary(&selection);
        Ok(())
    }

    fn first_project(&mut self) -> Result<Option<project::Checkout>> {
        let path = match &self.options.project {
            Some(path) => Some(path.clone()),
            None if self.options.yes => None,
            None => {
                let answer = self.prompter.text(
                    "Path to a repository checkout for GAH to work on (leave empty to add one later)",
                    None,
                )?;
                (!answer.trim().is_empty()).then(|| expand_home(answer.trim()))
            }
        };
        let Some(path) = path else { return Ok(None) };
        let checkout = project::inspect(&path)?;
        self.prompter.say(&format!(
            "✓ {} on {} (target branch {}).",
            checkout.repo, checkout.host, checkout.default_branch
        ));
        Ok(Some(checkout))
    }

    fn worker_settings(&mut self, env: &mut InstallEnv) -> Result<()> {
        self.prompter
            .say("A worker needs its central node's address and access token.");
        self.prompter
            .say("On the central node: Settings → Add a Node shows both.");
        let url = match self.options.central_url.clone() {
            Some(url) => url,
            None if self.options.yes => {
                bail!("--central-url is required for an unattended worker setup.")
            }
            None => self.prompter.text(
                "Central node address (for example https://central.your-tailnet.ts.net)",
                None,
            )?,
        };
        let url = url.trim().trim_end_matches('/').to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("The central address must start with http:// or https://.");
        }
        let token = self.secret_from("COORDINATOR_TOKEN", "Central access token (hidden)")?;
        match self
            .effects
            .http("GET", &format!("{url}/api/info"), Some(&token), None)
        {
            Some(200) => self
                .prompter
                .say("✓ The central node answered and accepted the token."),
            Some(401 | 403) => bail!(
                "The central node rejected that token. Copy it again from Settings → Add a Node."
            ),
            Some(status) => bail!("The central node answered HTTP {status}. Check the address."),
            None => {
                bail!("Nothing answered at {url}. Is this machine on the same network or tailnet?")
            }
        }
        env.0.push(("GAH_CENTRAL_URL", url));
        env.0.push(("COORDINATOR_TOKEN", token));
        Ok(())
    }

    fn memory_settings(&mut self, env: &mut InstallEnv) -> Result<MemoryMode> {
        let mode = match self.options.memory {
            Some(mode) => mode,
            None => {
                self.prompter.say(
                    "Shared memory lets chats and runs remember project context. It is optional:",
                );
                self.prompter.say(
                    "running it here needs a MemoryCore checkout and an OpenAI-compatible API key.",
                );
                let options = [
                    "Skip it for now (you can add it later)".to_string(),
                    "Run the memory gateway on this machine".to_string(),
                    "Use a memory gateway that already runs elsewhere".to_string(),
                ];
                [MemoryMode::Off, MemoryMode::Colocated, MemoryMode::Remote]
                    [self.ask_choice("Shared memory?", &options, 0)?]
            }
        };
        match mode {
            MemoryMode::Off => {}
            MemoryMode::Remote => {
                let url = match self.options.gateway_url.clone() {
                    Some(url) => url,
                    None if self.options.yes => {
                        bail!("--gateway-url is required for an unattended remote memory setup.")
                    }
                    None => self.prompter.text(
                        "Memory gateway address (for example http://central:8420)",
                        None,
                    )?,
                };
                let url = url.trim().trim_end_matches('/').to_string();
                let key =
                    self.secret_from("GAH_GATEWAY_API_KEY", "Memory gateway API key (hidden)")?;
                let body =
                    r#"{"query":"gah setup reachability check","session_key":"gah:setup-check"}"#;
                match self
                    .effects
                    .http("POST", &format!("{url}/recall"), Some(&key), Some(body))
                {
                    Some(200..=299) => self
                        .prompter
                        .say("✓ The memory gateway answered and accepted the key."),
                    Some(401 | 403) => bail!("The memory gateway rejected that key; fix TDAI_GATEWAY_API_KEY in ~/.config/gah/tdai-gateway.env and run setup again."),
                    Some(status) => bail!("The memory gateway answered HTTP {status}."),
                    None => bail!("Nothing answered at {url}/recall."),
                }
                env.0.extend([
                    ("GAH_GATEWAY_MODE", "remote".to_string()),
                    ("GAH_GATEWAY_URL", url),
                    ("GAH_GATEWAY_API_KEY", key),
                ]);
            }
            MemoryMode::Colocated => {
                let default = host::home().join("TencentDB-Agent-Memory/MemoryCore");
                let path = match self.options.memorycore.clone() {
                    Some(path) => path,
                    None if self.options.yes => default.clone(),
                    None => expand_home(
                        &self
                            .prompter
                            .text("MemoryCore checkout", Some(&default.display().to_string()))?,
                    ),
                };
                if !self.host.exists(&path.join("src/gateway/server.ts")) {
                    let repository = path.parent().unwrap_or(&path).to_path_buf();
                    let clone = format!(
                        "git clone https://github.com/Kh1ng/TencentDB-Agent-Memory.git {}",
                        shell_quote(&repository.display().to_string())
                    );
                    self.prompter
                        .say(&format!("No MemoryCore checkout at {}.", path.display()));
                    if !self.ask_confirm(&format!("Clone it with `{clone}`?"), true)?
                        || !self.effects.run(&clone, None, &[])
                    {
                        bail!("Shared memory needs a MemoryCore checkout. Clone it, then run `{}` again, or skip shared memory.", self.again());
                    }
                }
                if !self.host.exists(&path.join("node_modules")) {
                    let install = "npm install";
                    if self.ask_confirm(
                        &format!(
                            "Install MemoryCore's packages (`{install}` in {})?",
                            path.display()
                        ),
                        true,
                    )? && !self.effects.run(install, Some(&path), &[])
                    {
                        bail!("`npm install` failed in {}.", path.display());
                    }
                }
                // Model credentials belong to the selected gateway backend. A
                // configured local gateway may not need any generation API key.
                env.0.extend([
                    ("GAH_GATEWAY_MODE", "colocated".to_string()),
                    ("GAH_GATEWAY_MEMORYCORE_PATH", path.display().to_string()),
                ]);
                let existing_config = self.host.exists(&path.join("tdai-gateway.local.yaml"));
                let provider = match self.host.env("GAH_GATEWAY_PROVIDER") {
                    Some(val) => Some(val),
                    None if existing_config && self.options.yes => None,
                    None if self.options.yes => Some("ollama".to_string()),
                    None => {
                        if existing_config {
                            let opts = [
                                "Keep existing configuration".to_string(),
                                "Ollama (local, unmetered)".to_string(),
                                "OpenAI / Compatible".to_string(),
                            ];
                            match self.ask_choice(
                                "Select the model provider for MemoryCore",
                                &opts,
                                0,
                            )? {
                                0 => None,
                                2 => Some("openai".to_string()),
                                _ => Some("ollama".to_string()),
                            }
                        } else {
                            let opts = [
                                "Ollama (local, unmetered)".to_string(),
                                "OpenAI / Compatible".to_string(),
                            ];
                            match self.ask_choice(
                                "Select the model provider for MemoryCore",
                                &opts,
                                0,
                            )? {
                                1 => Some("openai".to_string()),
                                _ => Some("ollama".to_string()),
                            }
                        }
                    }
                };

                if let Some(provider) = provider {
                    env.0.push(("GAH_GATEWAY_PROVIDER", provider.clone()));

                    let (default_endpoint, default_llm, default_embed) = match provider.as_str() {
                        "ollama" => ("http://127.0.0.1:11434/v1", "llama3", "nomic-embed-text"),
                        _ => (
                            "https://api.openai.com/v1",
                            "gpt-4o",
                            "text-embedding-3-small",
                        ),
                    };

                    let endpoint = match self.host.env("GAH_GATEWAY_ENDPOINT") {
                        Some(val) => val,
                        None if self.options.yes => default_endpoint.to_string(),
                        None => {
                            let text = self.prompter.text(
                                &format!("API Endpoint (default: {default_endpoint})"),
                                None,
                            )?;
                            if text.is_empty() {
                                default_endpoint.to_string()
                            } else {
                                text
                            }
                        }
                    };
                    env.0.push(("GAH_GATEWAY_ENDPOINT", endpoint));

                    let llm_model = match self.host.env("GAH_GATEWAY_LLM_MODEL") {
                        Some(val) => val,
                        None if self.options.yes => default_llm.to_string(),
                        None => {
                            let text = self
                                .prompter
                                .text(&format!("LLM Model (default: {default_llm})"), None)?;
                            if text.is_empty() {
                                default_llm.to_string()
                            } else {
                                text
                            }
                        }
                    };
                    env.0.push(("GAH_GATEWAY_LLM_MODEL", llm_model));

                    let embed_model = match self.host.env("GAH_GATEWAY_EMBEDDING_MODEL") {
                        Some(val) => val,
                        None if self.options.yes => default_embed.to_string(),
                        None => {
                            let text = self.prompter.text(
                                &format!("Embedding Model (default: {default_embed})"),
                                None,
                            )?;
                            if text.is_empty() {
                                default_embed.to_string()
                            } else {
                                text
                            }
                        }
                    };
                    env.0.push(("GAH_GATEWAY_EMBEDDING_MODEL", embed_model));

                    let embed_dims = match self.host.env("GAH_GATEWAY_EMBEDDING_DIMENSIONS") {
                        Some(val) => Some(val),
                        None if self.options.yes => None,
                        None => {
                            let text = self.prompter.text(
                                "Embedding Dimensions (leave empty for known models)",
                                None,
                            )?;
                            if text.trim().is_empty() {
                                None
                            } else {
                                Some(text.trim().to_string())
                            }
                        }
                    };
                    if let Some(dims) = embed_dims {
                        env.0.push(("GAH_GATEWAY_EMBEDDING_DIMENSIONS", dims));
                    }

                    if provider == "openai" {
                        let llm_key = match self.host.env("GAH_GATEWAY_LLM_API_KEY") {
                            Some(val) => Some(val),
                            None => {
                                if self.options.yes {
                                    None
                                } else {
                                    let val = self
                                        .prompter
                                        .secret("LLM API key (hidden, empty to skip)")?;
                                    if val.trim().is_empty() {
                                        None
                                    } else {
                                        Some(val.trim().to_string())
                                    }
                                }
                            }
                        };
                        if let Some(val) = llm_key {
                            env.0.push(("GAH_GATEWAY_LLM_API_KEY", val));
                        }
                        let embed_key = match self.host.env("GAH_GATEWAY_EMBEDDING_API_KEY") {
                            Some(val) => val,
                            None => self.secret_from(
                                "GAH_GATEWAY_EMBEDDING_API_KEY",
                                "Embedding API key (hidden)",
                            )?,
                        };
                        env.0.push(("GAH_GATEWAY_EMBEDDING_API_KEY", embed_key));
                    } else {
                        if let Some(key) = self.host.env("GAH_GATEWAY_LLM_API_KEY") {
                            env.0.push(("GAH_GATEWAY_LLM_API_KEY", key));
                        }
                        if let Some(key) = self.host.env("GAH_GATEWAY_EMBEDDING_API_KEY") {
                            env.0.push(("GAH_GATEWAY_EMBEDDING_API_KEY", key));
                        }
                    }
                }
                self.prompter
                    .say("Gateway access uses a separate TDAI_GATEWAY_API_KEY.");
            }
        }
        Ok(mode)
    }

    /// Checks everything, offers each missing item in order, and re-checks
    /// after every action. Stops before any build if something required is
    /// still missing.
    fn provide_requirements(&mut self, selection: &Selection) -> Result<()> {
        let list = requirements::requirements(selection, self.host);
        self.prompter.say("");
        self.prompter.say("What this machine needs:");
        for requirement in &list {
            self.prompter.say(&line(requirement));
        }
        self.prompter.say("");
        let mut declined: Vec<&'static str> = Vec::new();
        loop {
            let list = requirements::requirements(selection, self.host);
            let ready = |id: &str| {
                list.iter()
                    .any(|other| other.id == id && other.status.is_ok())
            };
            let Some(next) = list.iter().find(|requirement| {
                !requirement.status.is_ok()
                    && !requirement.status.is_unresolved()
                    && requirement.action.is_some()
                    && !declined.contains(&requirement.id)
                    && requirement.needs().is_none_or(ready)
            }) else {
                break;
            };
            let action: &Action = next.action.as_ref().expect("filtered on action");
            let verb = if action.kind == ActionKind::Login {
                "Log in"
            } else {
                "Install"
            };
            let sudo_note = if action.sudo {
                " It asks for your password."
            } else {
                ""
            };
            self.prompter.say(&format!("{}: {}", next.label, next.why));
            let default = !next.optional;
            if self.ask_confirm(
                &format!("{verb} now with `{}`?{sudo_note}", action.command),
                default,
            )? {
                let command = action.command.clone();
                let id = next.id;
                if !self.effects.run(&command, None, &[]) {
                    self.prompter
                        .say(&format!("✗ `{command}` did not finish successfully."));
                    // A failed login stops setup (#1324): the next question
                    // would hide the failure behind unrelated prompts.
                    if action.kind == ActionKind::Login {
                        bail!(
                            "{} is still needed. Run `{command}` in this terminal and finish its prompts, then run `{}` again. It picks up where it stopped.",
                            next.label,
                            self.again()
                        );
                    }
                    declined.push(id);
                }
                host::refresh_path();
            } else {
                declined.push(next.id);
            }
        }
        let list = requirements::requirements(selection, self.host);
        for requirement in list.iter().filter(|r| r.status.is_unresolved()) {
            if let Status::StatusUnknown { reason } | Status::StatusFailed { reason } =
                &requirement.status
            {
                self.prompter.say(&format!(
                    "! {}: {reason} The login may still be valid, so setup continues. If GAH later reports a login error, run `{}` again to re-check it.",
                    requirement.label,
                    self.again()
                ));
            }
        }
        let blocking: Vec<&Requirement> = list
            .iter()
            .filter(|requirement| requirement.blocking())
            .collect();
        if blocking.is_empty() {
            self.prompter
                .say("✓ Everything this machine needs is in place.");
            return Ok(());
        }
        self.prompter
            .say("Still missing, so nothing was built yet:");
        for requirement in &blocking {
            let how = requirement
                .action
                .as_ref()
                .map(|action| format!("run `{}`", action.command))
                .or_else(|| requirement.help.map(|url| format!("see {url}")))
                .unwrap_or_else(|| match &requirement.status {
                    Status::Unsupported { reason } => reason.clone(),
                    _ => "install it".into(),
                });
            self.prompter
                .say(&format!("  ✗ {}: {how}", requirement.label));
        }
        bail!(
            "Provide the items above, then run `{}` again. It picks up where it stopped.",
            self.again()
        );
    }

    fn install(&mut self, selection: &Selection, env: InstallEnv) -> Result<()> {
        let source = self.options.source.clone();
        let (command, what) = match selection.role {
            Role::CliOnly => (
                // `cargo install --locked` re-resolves a stale Cargo.lock;
                // `cargo metadata --locked` fails on one, so it runs first.
                "cargo metadata --locked --format-version 1 >/dev/null && cargo install --path . --bin gah --force --locked".to_string(),
                "Build and install the gah command",
            ),
            Role::Central | Role::Standalone | Role::Worker => (
                "scripts/install.sh".to_string(),
                "Build GAH and install its background service",
            ),
        };
        if selection.role != Role::CliOnly {
            let role = match selection.role {
                Role::Worker => crate::update::HostRole::Worker,
                Role::Standalone => crate::update::HostRole::Standalone,
                _ => crate::update::HostRole::Central,
            };
            for change in
                crate::update::installation_plan(role, &[selection.agent.command().into()])?
            {
                self.prompter.say(&format!("  - {change}"));
            }
            self.prompter.say("  - Persist the selected node role and optional central URL in GAH configuration; write worker coordinator credentials to ~/.config/gah/gah-loop.env when supplied");
            if self.host.os() == Os::Linux {
                if matches!(selection.role, Role::Central | Role::Standalone) {
                    self.prompter.say("  - Use sudo to create /etc/gah/server.env if absent; enable and start gah-server.service");
                    if selection.role == Role::Standalone {
                        self.prompter.say("  - Set HOST to GAH_SERVER_HOST (default 127.0.0.1) in /etc/gah/server.env and restart gah-server.service");
                    } else {
                        self.prompter.say("  - For a new server.env, set HOST to GAH_SERVER_HOST or the Tailscale IP (fallback 127.0.0.1); preserve existing central HOST settings");
                    }
                }
                if selection.role != Role::Standalone {
                    self.prompter
                        .say("  - Enable Tailscale accept-dns when Tailscale is installed (sudo)");
                }
            } else {
                self.prompter.say("  - Install npm dependencies and build the role server; replace old role LaunchAgents");
                if selection.role == Role::Central {
                    self.prompter.say("  - Build MCP and web UI in the checkout; start the central server LaunchAgent");
                } else {
                    self.prompter.say("  - Configure worker identity and desktop settings under ~/.local/share/gah/worker and ~/.config/gah; install the worker LaunchAgent stopped until a profile is configured");
                }
                self.prompter
                    .say("  - Enable Tailscale accept-dns when Tailscale is installed");
            }
            if selection.memory != MemoryMode::Off {
                if self.host.os() == Os::Linux {
                    self.prompter.say("  - Configure gateway URL in ~/.config/gah/gah-loop.env and, for central/standalone, /etc/gah/server.env (sudo); store gateway credentials in ~/.config/gah/tdai-gateway.env");
                } else {
                    self.prompter.say("  - Store gateway credentials in ~/.config/gah/tdai-gateway.env and gateway URL in ~/.config/gah/server.env");
                }
                if selection.memory == MemoryMode::Colocated {
                    let path = env
                        .0
                        .iter()
                        .find(|(key, _)| *key == "GAH_GATEWAY_MEMORYCORE_PATH")
                        .map(|(_, value)| value.as_str())
                        .unwrap_or("<MemoryCore checkout>");
                    self.prompter.say(&format!("  - Seed {path}/tdai-gateway.local.yaml from the standalone template if absent; preserve an existing config"));
                    if self.host.os() == Os::Linux {
                        self.prompter.say("  - Install and enable/start user tdai-memory-gateway.service; check gateway health");
                    } else {
                        self.prompter.say("  - Install and start the memory-gateway LaunchAgent; check gateway health");
                    }
                } else {
                    self.prompter
                        .say("  - Check remote gateway reachability and authentication");
                }
            }
        }
        self.prompter.say("");
        if !self.ask_confirm(
            &format!("{what} now? The first build takes several minutes."),
            true,
        )? {
            bail!(
                "Setup stopped before building. Run `{}` again when you are ready.",
                self.again()
            );
        }
        let mut variables = env.0;
        // Do not let inherited installer options expand this setup selection.
        if !variables.iter().any(|(key, _)| *key == "GAH_GATEWAY_MODE") {
            variables.push(("GAH_GATEWAY_MODE", String::new()));
        }
        variables.push(("GAH_IMPORT_REPO", String::new()));
        variables.push(("GAH_INSTALL_AGENT", selection.agent.command().into()));
        variables.push(("GAH_INSTALL_CONFIRMED", "1".into()));
        variables.push((
            "GAH_NODE_ROLE",
            match selection.role {
                Role::Worker => "worker",
                Role::Standalone => "standalone",
                _ => "central",
            }
            .to_string(),
        ));
        if !self.effects.run(&command, Some(&source), &variables) {
            bail!("The install did not finish. Its output above says why; fix that and run `{}` again.", self.again());
        }
        host::refresh_path();
        self.prompter.say("✓ Installed.");
        Ok(())
    }

    fn summary(&mut self, selection: &Selection) {
        self.prompter.say("");
        self.prompter.say("Set up:");
        for feature in selection.features() {
            self.prompter.say(&format!("  ✓ {}", feature.label()));
        }
        let list = requirements::requirements(selection, self.host);
        let skipped: Vec<&Requirement> = list
            .iter()
            .filter(|requirement| requirement.optional && !requirement.status.is_ok())
            .collect();
        if !skipped.is_empty() || selection.memory == MemoryMode::Off {
            self.prompter.say("Skipped, and how to add it later:");
            for requirement in skipped {
                let how = requirement
                    .action
                    .as_ref()
                    .map(|action| action.command.clone())
                    .or(requirement.help.map(str::to_string))
                    .unwrap_or_default();
                self.prompter
                    .say(&format!("  · {}: {how}", requirement.label));
            }
            if matches!(selection.role, Role::Central | Role::Standalone)
                && selection.memory == MemoryMode::Off
            {
                self.prompter
                    .say("  · Shared memory: gah setup --memory colocated (or --memory remote)");
            }
        }
        self.prompter.say("Next:");
        match selection.role {
            Role::Central => {
                self.prompter.say("  · Open the dashboard at http://127.0.0.1:3773 (or this machine's tailnet address).");
                self.prompter
                    .say("  · Pair your phone from Settings → Pair a device.");
                self.prompter
                    .say("  · Add a worker from Settings → Add a Node.");
            }
            Role::Standalone => self
                .prompter
                .say("  · Open the dashboard at http://127.0.0.1:3773."),
            Role::Worker => self
                .prompter
                .say("  · This worker appears under Nodes on the central dashboard."),
            Role::CliOnly => self
                .prompter
                .say("  · Run `gah doctor`, then `gah dispatch --profile <project>`."),
        }
        self.prompter
            .say("Run `gah setup` again any time; it only does what is missing.");
    }
}

fn line(requirement: &Requirement) -> String {
    let (mark, state) = match &requirement.status {
        Status::Ok { found } => ("✓", found.clone().unwrap_or_default()),
        Status::Missing => (
            if requirement.optional { "·" } else { "✗" },
            "missing".into(),
        ),
        Status::Outdated { found } => ("✗", format!("too old ({found})")),
        Status::NotLoggedIn { reason }
        | Status::CredentialsRejected { reason }
        | Status::StatusUnknown { reason }
        | Status::StatusFailed { reason } => ("✗", reason.clone()),
        Status::Unsupported { reason } => ("✗", reason.clone()),
    };
    let optional = if requirement.optional {
        " (recommended)"
    } else {
        ""
    };
    if state.is_empty() {
        format!("  {mark} {}{optional}", requirement.label)
    } else {
        format!("  {mark} {}{optional}: {state}", requirement.label)
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => host::home().join(rest),
        None => PathBuf::from(path),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Finds the GAH checkout to build from: an explicit path, else the checkout
/// this binary was built in, else the current directory.
pub fn find_source(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let is_source =
        |dir: &Path| dir.join("scripts/install.sh").is_file() && dir.join("Cargo.toml").is_file();
    if let Some(dir) = explicit {
        return if is_source(&dir) {
            Ok(dir)
        } else {
            bail!("{} is not a git-agent-harness checkout", dir.display())
        };
    }
    let from_binary = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .find(|dir| is_source(dir))
            .map(Path::to_path_buf)
    });
    let from_cwd = std::env::current_dir().ok().filter(|dir| is_source(dir));
    from_binary
        .or(from_cwd)
        .context("Cannot find the git-agent-harness checkout to build. Pass --source <path>.")
}

#[cfg(test)]
mod tests;
