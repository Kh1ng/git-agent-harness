//! Guided first install (`gah setup`).
//!
//! - `requirements` is the one list of what each feature needs.
//! - `host` reads the machine; `project` turns a checkout into a profile.
//! - `wizard` asks, checks, offers, installs, and summarizes.

pub mod host;
pub mod project;
pub mod requirements;
pub mod wizard;

use anyhow::{bail, Result};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Prompts on the terminal. Hidden input turns echo off with `stty`.
pub struct TerminalPrompter;

fn read_line() -> Result<String> {
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        bail!("Input ended before setup finished.");
    }
    Ok(line.trim().to_string())
}

impl wizard::Prompter for TerminalPrompter {
    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        println!("{question}");
        for (index, option) in options.iter().enumerate() {
            println!(
                "  {}) {option}{}",
                index + 1,
                if index == default { "  [default]" } else { "" }
            );
        }
        loop {
            print!("Choose 1-{} [{}]: ", options.len(), default + 1);
            std::io::stdout().flush()?;
            let answer = read_line()?;
            if answer.is_empty() {
                return Ok(default);
            }
            match answer.parse::<usize>() {
                Ok(choice) if (1..=options.len()).contains(&choice) => return Ok(choice - 1),
                _ => println!("Enter a number from the list."),
            }
        }
    }

    fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        loop {
            print!("{question} [{}] ", if default { "Y/n" } else { "y/N" });
            std::io::stdout().flush()?;
            match read_line()?.to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => println!("Answer y or n."),
            }
        }
    }

    fn text(&mut self, question: &str, default: Option<&str>) -> Result<String> {
        match default {
            Some(default) => print!("{question} [{default}]: "),
            None => print!("{question}: "),
        }
        std::io::stdout().flush()?;
        let answer = read_line()?;
        Ok(if answer.is_empty() {
            default.unwrap_or_default().to_string()
        } else {
            answer
        })
    }

    fn secret(&mut self, question: &str) -> Result<String> {
        print!("{question}: ");
        std::io::stdout().flush()?;
        let echo = |on: bool| {
            let _ = Command::new("stty")
                .arg(if on { "echo" } else { "-echo" })
                .stdin(Stdio::inherit())
                .status();
        };
        echo(false);
        let answer = read_line();
        echo(true);
        println!();
        answer
    }
}

/// The real machine: shell commands with the terminal attached, HTTP through
/// curl (credentials in its stdin config, never argv), and the GAH config.
pub struct SystemEffects;

impl wizard::Effects for SystemEffects {
    fn run(&mut self, command: &str, cwd: Option<&Path>, env: &[(&str, String)]) -> bool {
        println!("$ {command}");
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        for (key, value) in env {
            process.env(key, value);
        }
        process
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn http(
        &mut self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> Option<u16> {
        crate::curl_http::request(method, url, body, bearer, 10)
            .ok()
            .map(|response| response.status)
    }

    fn profile_exists(&mut self, profile: &str) -> bool {
        std::fs::read_to_string(crate::config::resolve_config_path(None))
            .map(|config| config.contains(&format!("[profiles.{profile}]")))
            .unwrap_or(false)
    }

    fn add_profile(&mut self, args: crate::init::InitArgs) -> Result<()> {
        crate::init::run(args)
    }
}

pub struct CheckArgs {
    pub json: bool,
}

/// `gah setup`: the guided install, or `--check` for a read-only report.
pub fn run(options: wizard::Options, check: Option<CheckArgs>) -> Result<()> {
    let host = host::SystemHost;
    if let Some(check) = check {
        let selection = requirements::Selection {
            role: options.role.unwrap_or(requirements::Role::Central),
            agent: options.agent.unwrap_or(requirements::Agent::Claude),
            provider: options.provider.unwrap_or(requirements::Provider::Github),
            memory: options.memory.unwrap_or(requirements::MemoryMode::Off),
        };
        let report = requirements::report(selection, &host);
        if check.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            for requirement in &report.requirements {
                let mark = if requirement.status.is_ok() {
                    "✓"
                } else if requirement.optional {
                    "·"
                } else {
                    "✗"
                };
                println!("{mark} {}: {}", requirement.label, requirement.why);
            }
            println!(
                "{}",
                if report.ready {
                    "Ready."
                } else {
                    "Not ready: run `gah setup` to provide what is missing."
                }
            );
        }
        return Ok(());
    }
    if !options.yes && !std::io::stdin().is_terminal() {
        bail!("gah setup asks questions; run it in a terminal, or pass --yes with the choices as flags.");
    }
    let mut prompter = TerminalPrompter;
    let mut effects = SystemEffects;
    wizard::Setup {
        host: &host,
        prompter: &mut prompter,
        effects: &mut effects,
        options,
    }
    .run()
}
