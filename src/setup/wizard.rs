//! `gah setup`: the guided install. It asks what the machine is for, checks
//! every prerequisite before anything is built, offers to provide what is
//! missing (never without a yes), installs the service through
//! `scripts/install.sh`, adds the first project, and ends with what was set
//! up and how to add the rest later.

use super::host::{self, Host};
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
    pub factory_module: Option<bool>,
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
        let role = match self.options.role {
            Some(role) => role,
            None => {
                let options = [
                    "My main machine: dashboard, chats, and phone control (central node)"
                        .to_string(),
                    "A worker that runs jobs for a central node I already have".to_string(),
                    "Just the command line, no server".to_string(),
                ];
                [Role::Central, Role::Worker, Role::CliOnly]
                    [self.ask_choice("What is this machine for?", &options, 0)?]
            }
        };

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
        if let Some(enabled) = self.options.factory_module {
            env.0.push((
                "GAH_FACTORY_MODULE",
                if enabled { "1" } else { "0" }.to_string(),
            ));
        } else if let Ok(val) = std::env::var("GAH_FACTORY_MODULE") {
            env.0.push(("GAH_FACTORY_MODULE", val));
        }
        let mut memory = MemoryMode::Off;
        match role {
            Role::Worker => self.worker_settings(&mut env)?,
            Role::Central => memory = self.memory_settings(&mut env)?,
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
                let llm = self.secret_from(
                    "GAH_GATEWAY_LLM_API_KEY",
                    "OpenAI-compatible API key for the memory gateway (hidden)",
                )?;
                env.0.extend([
                    ("GAH_GATEWAY_MODE", "colocated".to_string()),
                    ("GAH_GATEWAY_MEMORYCORE_PATH", path.display().to_string()),
                    ("GAH_GATEWAY_LLM_API_KEY", llm),
                ]);
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
                    declined.push(id);
                }
                host::refresh_path();
            } else {
                declined.push(next.id);
            }
        }
        let list = requirements::requirements(selection, self.host);
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
                "cargo install --path . --force".to_string(),
                "Build and install the gah command",
            ),
            Role::Central | Role::Worker => (
                "scripts/install.sh".to_string(),
                "Build GAH and install its background service",
            ),
        };
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
        variables.push((
            "GAH_NODE_ROLE",
            match selection.role {
                Role::Worker => "worker",
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
            if selection.role == Role::Central && selection.memory == MemoryMode::Off {
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
        Status::NotLoggedIn => ("✗", "not logged in".into()),
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
mod tests {
    use super::*;
    use crate::setup::host::{Os, PackageManager};
    use crate::setup::requirements::tests::FakeHost;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Script {
        answers: VecDeque<String>,
        said: Vec<String>,
        asked: Vec<String>,
    }

    impl Prompter for Script {
        fn say(&mut self, line: &str) {
            self.said.push(line.to_string());
        }
        fn choose(
            &mut self,
            question: &str,
            _options: &[String],
            _default: usize,
        ) -> Result<usize> {
            self.asked.push(question.to_string());
            Ok(self
                .answers
                .pop_front()
                .context("unexpected question")?
                .parse()?)
        }
        fn confirm(&mut self, question: &str, _default: bool) -> Result<bool> {
            self.asked.push(question.to_string());
            Ok(self
                .answers
                .pop_front()
                .context("unexpected confirmation")?
                == "y")
        }
        fn text(&mut self, question: &str, _default: Option<&str>) -> Result<String> {
            self.asked.push(question.to_string());
            self.answers.pop_front().context("unexpected text")
        }
        fn secret(&mut self, question: &str) -> Result<String> {
            self.asked.push(question.to_string());
            self.answers.pop_front().context("unexpected secret")
        }
    }

    #[derive(Default)]
    struct Recorder {
        commands: Vec<(String, Vec<(String, String)>)>,
        http: Vec<(String, String)>,
        status: Option<u16>,
        profiles: Vec<String>,
    }

    impl Effects for Recorder {
        fn run(&mut self, command: &str, _cwd: Option<&Path>, env: &[(&str, String)]) -> bool {
            self.commands.push((
                command.to_string(),
                env.iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            ));
            true
        }
        fn http(
            &mut self,
            method: &str,
            url: &str,
            _bearer: Option<&str>,
            _body: Option<&str>,
        ) -> Option<u16> {
            self.http.push((method.to_string(), url.to_string()));
            self.status
        }
        fn profile_exists(&mut self, profile: &str) -> bool {
            self.profiles.iter().any(|p| p == profile)
        }
        fn add_profile(&mut self, args: InitArgs) -> Result<()> {
            self.profiles.push(args.profile);
            Ok(())
        }
    }

    fn ready_host() -> FakeHost {
        FakeHost::new(Os::Linux, Some(PackageManager::Apt))
            .with("git --version", true, "git version 2.43.0")
            .with("cargo --version", true, "cargo 1.94.1")
            .with("node --version", true, "v22.4.0")
            .with("claude --version", true, "2.1.0")
            .with("claude auth status --json", true, r#"{"loggedIn":true}"#)
            .with("gh --version", true, "gh version 2.60.0")
            .with(
                "gh auth status",
                true,
                "✓ Logged in to github.com account octo",
            )
            .with("curl --version", true, "curl 8.5.0")
            .with("systemctl --version", true, "systemd 255")
            .with("tailscale version", true, "1.76.0")
    }

    #[test]
    fn a_ready_central_machine_asks_three_questions_then_installs() {
        let host = ready_host();
        let mut prompter = Script {
            answers: ["0", "", "0", "0", "0", "y"].map(String::from).into(),
            ..Default::default()
        };
        let mut effects = Recorder::default();
        Setup {
            host: &host,
            prompter: &mut prompter,
            effects: &mut effects,
            options: Options {
                provider: None,
                source: PathBuf::from("/src"),
                ..Default::default()
            },
        }
        .run()
        .unwrap();
        assert_eq!(effects.commands.len(), 1);
        let (command, env) = &effects.commands[0];
        assert_eq!(command, "scripts/install.sh");
        assert!(env.contains(&("GAH_NODE_ROLE".into(), "central".into())));
        assert!(
            prompter
                .said
                .iter()
                .any(|line| line.contains("Shared memory: gah setup --memory")),
            "skipped memory says how to add it"
        );
    }

    #[test]
    fn a_missing_prerequisite_is_offered_and_a_declined_one_stops_before_building() {
        let host = FakeHost::new(Os::Linux, Some(PackageManager::Apt));
        let options = Options {
            role: Some(Role::CliOnly),
            agent: Some(Agent::Claude),
            provider: Some(Provider::Github),
            source: PathBuf::from("/src"),
            ..Default::default()
        };
        // Blank project path, then decline every offer.
        let mut prompter = Script {
            answers: std::iter::once(String::new())
                .chain(std::iter::repeat_n("n".to_string(), 10))
                .collect(),
            ..Default::default()
        };
        let mut effects = Recorder::default();
        let error = Setup {
            host: &host,
            prompter: &mut prompter,
            effects: &mut effects,
            options,
        }
        .run()
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("run `/src/target/release/gah setup` again"),
            "before gah is installed, the hint names the binary the paste script built"
        );
        assert!(
            effects.commands.is_empty(),
            "nothing is installed or built without a yes"
        );
        assert!(prompter
            .asked
            .iter()
            .any(|q| q.contains("sudo apt-get install -y git") && q.contains("password")));
        assert!(prompter.said.iter().any(|line| line.contains("✗ git")));
        assert!(
            !prompter
                .asked
                .iter()
                .any(|q| q.contains("npm install") || q.contains("auth login")),
            "nothing is offered before what it needs: npm needs Node, a login needs its program"
        );
    }

    #[test]
    fn a_worker_checks_its_central_node_before_building() {
        let host = ready_host();
        let mut prompter = Script {
            answers: ["1", "0", "0", "https://central.example/", "token-123", "y"]
                .map(String::from)
                .into(),
            ..Default::default()
        };
        let mut effects = Recorder {
            status: Some(200),
            ..Default::default()
        };
        Setup {
            host: &host,
            prompter: &mut prompter,
            effects: &mut effects,
            options: Options {
                source: PathBuf::from("/src"),
                ..Default::default()
            },
        }
        .run()
        .unwrap();
        assert_eq!(
            effects.http,
            [(
                "GET".to_string(),
                "https://central.example/api/info".to_string()
            )]
        );
        let env = &effects.commands[0].1;
        assert!(env.contains(&("GAH_NODE_ROLE".into(), "worker".into())));
        assert!(env.contains(&("GAH_CENTRAL_URL".into(), "https://central.example".into())));
        assert!(env.contains(&("COORDINATOR_TOKEN".into(), "token-123".into())));

        let mut prompter = Script {
            answers: ["1", "0", "0", "https://central.example", "bad"]
                .map(String::from)
                .into(),
            ..Default::default()
        };
        let mut effects = Recorder {
            status: Some(401),
            ..Default::default()
        };
        let error = Setup {
            host: &host,
            prompter: &mut prompter,
            effects: &mut effects,
            options: Options {
                source: PathBuf::from("/src"),
                ..Default::default()
            },
        }
        .run()
        .unwrap_err();
        assert!(error.to_string().contains("rejected that token"));
        assert!(effects.commands.is_empty());
    }

    #[test]
    fn unattended_setup_needs_secrets_from_the_environment() {
        let host = ready_host();
        let mut prompter = Script::default();
        let mut effects = Recorder::default();
        let options = Options {
            role: Some(Role::Worker),
            central_url: Some("https://c.example".into()),
            yes: true,
            source: PathBuf::from("/src"),
            ..Default::default()
        };
        let error = Setup {
            host: &host,
            prompter: &mut prompter,
            effects: &mut effects,
            options,
        }
        .run()
        .unwrap_err();
        assert!(error.to_string().contains("COORDINATOR_TOKEN is required"));
        assert!(prompter.asked.is_empty(), "--yes never prompts");
    }
}
