//! Reusable hermetic fake-backend test harness.
//!
//! Lives under `tests/support/` (not a top-level file under `tests/`) so
//! cargo does not treat it as its own test binary — it's pulled in via
//! `mod support;` from whichever integration test file needs it.
//!
//! Name-agnostic: the harness writes a POSIX shell script named after
//! whatever `name` you pass `FakeBackend::new`, so it works for any
//! executable GAH shells out to. It is explicitly exercised in
//! `tests/fake_backend_harness.rs` for the five backends GAH cares about
//! today: `openhands`, `opencode`, `claude`,
//! `codex`, `agy`.
//!
//! Every `FakeBackend` is an independently configured *instance*, not a
//! global fake keyed by executable name. Future availability/quota routing
//! needs to distinguish separate accounts on the same backend type (e.g.
//! two Claude subscriptions) — so state (call counter, recorded argv/env
//! per call) lives under a directory unique to the instance you construct,
//! never shared with any other instance, even one with the same `name`.
//! Combined with per-`Command` PATH injection (rather than mutating the
//! process-global PATH — see provider.rs's test seam for why that matters),
//! two instances of the same backend name can be exercised independently
//! within the same test process without interfering with each other or
//! with unrelated tests running in parallel.

#![allow(dead_code)] // this file is shared support code, not a test binary — individual helpers are used non-uniformly across consumers

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// One scripted response for a single invocation of a fake backend.
#[derive(Debug, Clone, Default)]
pub struct Scenario {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub delay_ms: u64,
}

impl Scenario {
    pub fn success() -> Self {
        Scenario::default()
    }

    pub fn failure(exit_code: i32) -> Self {
        Scenario {
            exit_code,
            ..Default::default()
        }
    }

    pub fn with_stdout(mut self, s: impl Into<String>) -> Self {
        self.stdout = s.into();
        self
    }

    pub fn with_stderr(mut self, s: impl Into<String>) -> Self {
        self.stderr = s.into();
        self
    }

    pub fn with_delay_ms(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

/// A single independently configured fake backend instance.
pub struct FakeBackend {
    name: String,
    bin_dir: PathBuf,
    record_dir: PathBuf,
    capture_env: Vec<String>,
}

impl FakeBackend {
    /// `root` should be a directory unique to this instance (e.g. a
    /// distinct subpath of a tempdir) — callers wanting N independent
    /// instances of the same backend name just pass N different roots.
    pub fn new(root: &Path, name: &str) -> Self {
        let bin_dir = root.join("bin");
        let record_dir = root.join("record");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&record_dir).unwrap();
        FakeBackend {
            name: name.to_string(),
            bin_dir,
            record_dir,
            capture_env: Vec::new(),
        }
    }

    /// Directory to prepend to PATH so `name` resolves to this instance.
    /// Prepend it to a `Command`'s own PATH env (not the process-global
    /// PATH) to keep instances — and unrelated parallel tests — isolated.
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    pub fn path_with(&self, existing_path: &str) -> String {
        format!("{}:{}", self.bin_dir.display(), existing_path)
    }

    /// Restrict captured environment to just these variable names. Without
    /// calling this, no environment is captured (argv still is).
    pub fn capture_env_vars(mut self, vars: &[&str]) -> Self {
        self.capture_env = vars.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Install a single fixed scenario: every call gets the same response.
    pub fn install(&self, scenario: Scenario) {
        self.install_sequence(vec![scenario]);
    }

    /// Install a deterministic sequence of scenarios: call 1 gets
    /// `sequence[0]`, call 2 gets `sequence[1]`, etc. Calls beyond the
    /// sequence length repeat the last scenario, so tests don't need to
    /// predict exactly how many calls will happen.
    pub fn install_sequence(&self, sequence: Vec<Scenario>) {
        assert!(!sequence.is_empty(), "scenario sequence must not be empty");
        let counter_path = self.record_dir.join("call-count");
        let _ = fs::remove_file(&counter_path);

        let mut script = String::from("#!/bin/sh\n");
        script.push_str(&format!(
            "n=$( [ -f '{c}' ] && cat '{c}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{c}'\n",
            c = counter_path.display(),
        ));
        script.push_str(&format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}/argv-call-'\"$n\"'.txt'\n",
            self.record_dir.display()
        ));
        if !self.capture_env.is_empty() {
            script.push_str(&format!(
                "> '{}/env-call-'\"$n\"'.txt'\n",
                self.record_dir.display()
            ));
            for var in &self.capture_env {
                // `set` / direct variable expansion, never the external
                // `env` binary: tests deliberately restrict PATH to just
                // the fake bin dir, so `env` itself may not be found.
                script.push_str(&format!(
                    "eval \"v=\\${var}\"; printf '{var}=%s\\n' \"$v\" >> '{dir}/env-call-'\"$n\"'.txt'\n",
                    var = var,
                    dir = self.record_dir.display(),
                ));
            }
        }
        script.push_str("case \"$n\" in\n");
        for (i, s) in sequence.iter().enumerate() {
            script.push_str(&format!("  {})\n", i + 1));
            script.push_str(&Self::render_case_body(s));
            script.push_str("  ;;\n");
        }
        // Beyond the scripted sequence: repeat the last scenario.
        script.push_str("  *)\n");
        script.push_str(&Self::render_case_body(sequence.last().unwrap()));
        script.push_str("  ;;\nesac\n");

        let path = self.bin_dir.join(&self.name);
        fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
    }

    fn render_case_body(s: &Scenario) -> String {
        let mut body = String::new();
        if s.delay_ms > 0 {
            let secs = s.delay_ms as f64 / 1000.0;
            body.push_str(&format!("    sleep {secs}\n"));
        }
        if !s.stdout.is_empty() {
            body.push_str(&format!(
                "    cat <<'GAH_FAKE_STDOUT_EOF'\n{}\nGAH_FAKE_STDOUT_EOF\n",
                s.stdout
            ));
        }
        if !s.stderr.is_empty() {
            body.push_str(&format!(
                "    cat >&2 <<'GAH_FAKE_STDERR_EOF'\n{}\nGAH_FAKE_STDERR_EOF\n",
                s.stderr
            ));
        }
        body.push_str(&format!("    exit {}\n", s.exit_code));
        body
    }

    /// How many times this instance has actually been invoked so far.
    pub fn call_count(&self) -> u32 {
        fs::read_to_string(self.record_dir.join("call-count"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Argv (excluding argv[0]) recorded for a specific 1-indexed call.
    pub fn argv_for_call(&self, call: u32) -> Vec<String> {
        fs::read_to_string(self.record_dir.join(format!("argv-call-{call}.txt")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Captured environment for a specific 1-indexed call, restricted to
    /// whatever `capture_env_vars` requested.
    pub fn env_for_call(&self, call: u32) -> HashMap<String, String> {
        fs::read_to_string(self.record_dir.join(format!("env-call-{call}.txt")))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

pub mod fake_ledger;
pub mod scenario;
