//! Deterministic controller/supervisor test harness for GAH.
//!
//! `ScenarioHarness` owns a hermetic environment — temp dir, config, fake
//! provider/worker scripts, ledger — and runs the compiled `gah` binary as a
//! subprocess against the real production path, with all external
//! processes (gh/glab/workers) intercepted by fake shell scripts.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use tempfile::TempDir;

use super::fake_ledger::TestLedger;
use super::{FakeBackend, Scenario};

static SERIAL: Mutex<()> = Mutex::new(());

/// Result of a single subprocess invocation.
#[derive(Debug)]
pub struct LoopResult {
    pub action_kind: String,
    pub action_details: String,
    pub exit_code: Option<i32>,
    pub stderr_tail: String,
    pub call_counts: HashMap<String, u32>,
    pub ledger_entries: Vec<serde_json::Value>,
    pub events: Vec<serde_json::Value>,
}

/// Deterministic test harness.
pub struct ScenarioHarness {
    _temp: TempDir,
    _lock: MutexGuard<'static, ()>,
    pub config_path: PathBuf,
    pub bin_dir: PathBuf,
    pub ledger_path: PathBuf,
    pub events_path: PathBuf,
    pub artifacts_dir: PathBuf,
    pub local_repo_dir: PathBuf,
    pub profile_name: String,
    pub provider: String,
    fake_gh: Option<FakeBackend>,
    fake_glab: Option<FakeBackend>,
    fake_workers: HashMap<String, FakeBackend>,
    ledger: TestLedger,
    gah_bin: PathBuf,
}

impl ScenarioHarness {
    pub fn new(provider: &str) -> Self {
        let lock = match SERIAL.lock() {
            Ok(l) => l,
            Err(e) => e.into_inner(),
        };
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let artifacts_dir = root.join("artifacts");
        let local_repo_dir = root.join("repo");
        let bin_dir = root.join("bin");
        let config_dir = root.join("config");
        fs::create_dir_all(&artifacts_dir).unwrap();
        fs::create_dir_all(&local_repo_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        let _ = Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&local_repo_dir)
            .output();

        let ledger_path = artifacts_dir.join("ledger.jsonl");
        let events_path = artifacts_dir.join("events.jsonl");
        fs::write(&ledger_path, "").unwrap();
        fs::write(&events_path, "").unwrap();

        let gah_bin = env::var("CARGO_BIN_EXE_gah")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("target/debug/gah"));

        Self {
            _temp: temp,
            _lock: lock,
            config_path: config_dir.join("config.toml"),
            bin_dir,
            ledger_path,
            events_path,
            artifacts_dir,
            local_repo_dir,
            profile_name: "test".to_string(),
            provider: provider.to_string(),
            fake_gh: None,
            fake_glab: None,
            fake_workers: HashMap::new(),
            ledger: TestLedger::new(),
            gah_bin,
        }
    }

    pub fn github_scenario(mut self, name: &str) -> Self {
        let fb = FakeBackend::new(self._temp.path(), "gh");
        fb.install(load_github_fixture(name));
        self.fake_gh = Some(fb);
        self
    }

    pub fn gitlab_scenario(mut self, name: &str) -> Self {
        let fb = FakeBackend::new(self._temp.path(), "glab");
        fb.install(load_gitlab_fixture(name));
        self.fake_glab = Some(fb);
        self
    }

    pub fn worker_scenario(mut self, name: &str) -> Self {
        let fixture = load_worker_fixture(name);
        for worker_name in &["openhands", "vibe", "opencode", "claude", "codex", "agy"] {
            let fb = FakeBackend::new(self._temp.path(), worker_name);
            fb.install(fixture.clone());
            self.fake_workers.insert(worker_name.to_string(), fb);
        }
        self
    }

    pub fn with_ledger(mut self, ledger: TestLedger) -> Self {
        self.ledger = ledger;
        self
    }

    fn setup_env(&self) {
        self.write_config();
        let _ = self.ledger.write_to(&self.ledger_path);

        env::set_var("GAH_CONFIG", self.config_path.to_str().unwrap());
        env::set_var("GAH_LEDGER_PATH", self.ledger_path.to_str().unwrap());
        env::set_var("GAH_EVENTS_PATH", self.events_path.to_str().unwrap());

        let new_path = format!(
            "{}:{}",
            self.bin_dir.display(),
            env::var("PATH").unwrap_or_default()
        );
        env::set_var("PATH", &new_path);
    }

    /// Run one loop iteration by spawning the `gah` binary.
    pub fn run_one_loop(&mut self) -> Result<LoopResult, String> {
        self.setup_env();

        // Ensure fake gh/glab scripts are in the bin_dir
        self.install_fakes();

        let out = Command::new(&self.gah_bin)
            .args([
                "loop",
                "--once",
                "--profile",
                &self.profile_name,
                "--json",
                "--skip-validation-gate",
            ])
            .env("GAH_CONFIG", self.config_path.to_str().unwrap())
            .env("GAH_LEDGER_PATH", self.ledger_path.to_str().unwrap())
            .env("GAH_EVENTS_PATH", self.events_path.to_str().unwrap())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin_dir.display(),
                    env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .map_err(|e| format!("spawn failed: {e}"))?;

        let exit_code = out.status.code();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr_tail = String::from_utf8_lossy(&out.stderr)
            .chars()
            .rev()
            .take(400)
            .collect::<String>()
            .chars()
            .rev()
            .collect();

        let (action_kind, action_details) = parse_action_from_stdout(&stdout);

        let mut call_counts = HashMap::new();
        if let Some(ref fb) = self.fake_gh {
            call_counts.insert("gh".into(), fb.call_count());
        }
        if let Some(ref fb) = self.fake_glab {
            call_counts.insert("glab".into(), fb.call_count());
        }
        for (name, fb) in &self.fake_workers {
            let c = fb.call_count();
            if c > 0 {
                call_counts.insert(name.clone(), c);
            }
        }

        let ledger_entries = TestLedger::read_from(&self.ledger_path).unwrap_or_default();
        let events = read_jsonl_lines(&self.events_path).unwrap_or_default();

        Ok(LoopResult {
            action_kind,
            action_details,
            exit_code,
            stderr_tail,
            call_counts,
            ledger_entries,
            events,
        })
    }

    pub fn run_loops(&mut self, n: usize) -> Result<Vec<LoopResult>, String> {
        let mut results = Vec::with_capacity(n);
        for _ in 0..n {
            results.push(self.run_one_loop()?);
        }
        Ok(results)
    }

    fn install_fakes(&self) {
        // gh/glab scripts need to be in the bin_dir since the gah
        // subprocess resolves them from PATH, not from the thread-local
        // provider override.
        if let Some(ref fb) = self.fake_gh {
            copy_script_to(fb, &self.bin_dir, "gh");
        }
        if let Some(ref fb) = self.fake_glab {
            copy_script_to(fb, &self.bin_dir, "glab");
        }
        for (name, fb) in &self.fake_workers {
            copy_script_to(fb, &self.bin_dir, name);
        }
    }

    fn write_config(&self) {
        let toml = format!(
            r#"[defaults]
artifact_root = "{artifacts}"
worktree_base = "{worktree}"

[profiles.{name}]
display_name = "Test Repo"
repo_id = "{name}"
provider = "{provider}"
repo = "{repo}"
local_path = "{local}"
artifact_root = "{artifacts}"
default_target_branch = "main"
"#,
            artifacts = self.artifacts_dir.display(),
            worktree = self._temp.path().join("worktrees").display(),
            name = self.profile_name,
            provider = self.provider,
            repo = if self.provider == "github" {
                "owner/repo"
            } else {
                "group/repo"
            },
            local = self.local_repo_dir.display(),
        );
        fs::write(&self.config_path, toml).unwrap();
    }
}

impl Drop for ScenarioHarness {
    fn drop(&mut self) {
        env::remove_var("GAH_CONFIG");
        env::remove_var("GAH_LEDGER_PATH");
        env::remove_var("GAH_EVENTS_PATH");
    }
}

// --- helpers ---

fn copy_script_to(fb: &FakeBackend, bin_dir: &Path, name: &str) {
    let src = fb.bin_dir().join(name);
    let dst = bin_dir.join(name);
    if src == dst {
        return; // ponytail: same dir, already installed
    }
    if src.exists() {
        let _ = fs::copy(&src, &dst);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&dst) {
                let mut p = meta.permissions();
                p.set_mode(0o755);
                let _ = fs::set_permissions(&dst, p);
            }
        }
    }
}

fn parse_action_from_stdout(stdout: &str) -> (String, String) {
    // gah loop --once --json outputs a final JSON object with action + outcome.
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(a) = v.get("action") {
                    let kind = a["type"].as_str().unwrap_or("unknown").to_string();
                    let details = a["reason"]
                        .as_str()
                        .unwrap_or(a["reference"].as_str().unwrap_or(""))
                        .to_string();
                    return (kind, details);
                }
            }
        }
    }
    ("exit_only".into(), stdout.chars().take(200).collect())
}

pub fn read_jsonl_lines(path: &Path) -> std::io::Result<Vec<serde_json::Value>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

// --- fixture loaders ---

fn load_github_fixture(name: &str) -> Scenario {
    match name {
        "empty" => Scenario::success().with_stdout("[]"),
        "malformed" => Scenario::success().with_stdout("not json"),
        "non_zero_exit" => Scenario::failure(1),
        "prs_closed_null_rollup" => Scenario::success().with_stdout(
            r#"[{"number":1,"title":"test","state":"CLOSED","statusCheckRollup":null}]"#,
        ),
        _ => Scenario::failure(127).with_stderr(format!("unknown github scenario: {name}")),
    }
}

fn load_gitlab_fixture(name: &str) -> Scenario {
    match name {
        "empty" => Scenario::success().with_stdout("[]"),
        "malformed" => Scenario::success().with_stdout("not json"),
        "non_zero_exit" => Scenario::failure(1),
        _ => Scenario::failure(127).with_stderr(format!("unknown gitlab scenario: {name}")),
    }
}

fn load_worker_fixture(name: &str) -> Scenario {
    match name {
        "success" => Scenario::success().with_stdout("work complete\n"),
        "failure" => Scenario::failure(1).with_stderr("worker failed"),
        "empty_success" => Scenario::success(),
        "invalid_output" => Scenario::success().with_stdout("unexpected output"),
        "hang" => Scenario {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            delay_ms: 300_000,
        },
        _ => Scenario::failure(127).with_stderr(format!("unknown worker scenario: {name}")),
    }
}
