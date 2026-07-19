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

use tempfile::TempDir;

use super::fake_ledger::TestLedger;
use super::{ExecGuard, FakeBackend, Scenario};

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

#[derive(Debug)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Deterministic test harness.
pub struct ScenarioHarness {
    _temp: TempDir,
    // Also serializes this harness's `gah` subprocess fork+exec against
    // any other test in this binary that writes+execs a temp fake-backend
    // script concurrently -- see `ExecGuard`'s doc comment in `mod.rs`.
    _lock: ExecGuard,
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
    config_append: String,
    // Environment capture/restore
    saved_path: Option<String>,
    saved_gah_config: Option<String>,
    saved_gah_ledger: Option<String>,
    saved_gah_events: Option<String>,
    saved_xdg_state_home: Option<String>,
}

impl ScenarioHarness {
    pub fn new(provider: &str) -> Self {
        let lock = ExecGuard::new();
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

        let _ = std::process::Command::new("git")
            .arg("init")
            .arg(&local_repo_dir)
            .output();

        // Non-bare repo with a commit, pointing origin at a separate bare
        // repo so `git fetch origin main` works (gah needs this for
        // branch operations during review/fix/merge dispatch).
        {
            let r = local_repo_dir.to_str().unwrap();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "config", "user.email", "test@test"])
                .output();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "config", "user.name", "test"])
                .output();
            std::fs::write(local_repo_dir.join(".gitkeep"), "").ok();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "add", ".gitkeep"])
                .output();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "commit", "-m", "initial"])
                .output();
            // Rename to `main` — gah config has `default_target_branch = "main"`.
            let _ = std::process::Command::new("git")
                .args(["-C", r, "branch", "-M", "main"])
                .output();

            // Bare clone to act as the "github" remote.
            let remote_dir = root.join("remote.git");
            let rr = remote_dir.to_str().unwrap();
            let _ = std::process::Command::new("git")
                .args(["clone", "--bare", r, rr])
                .output();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "remote", "add", "origin", rr])
                .output();
            let _ = std::process::Command::new("git")
                .args(["-C", r, "push", "-q", "origin", "main"])
                .output();
        }

        let ledger_path = artifacts_dir.join("ledger.jsonl");
        let events_path = artifacts_dir.join("events.jsonl");
        fs::write(&ledger_path, "").unwrap();
        fs::write(&events_path, "").unwrap();

        // Capture environment state at construction time so Drop can
        // always restore correctly — even if setup_env panics before
        // running its capture block.  Otherwise, a partial-harness panic
        // leaves saved_* at None, and restore_var(None) → remove_var,
        // which strips PATH from the process and poisons subsequent tests.
        let saved_path = env::var("PATH").ok();
        let saved_gah_config = env::var("GAH_CONFIG").ok();
        let saved_gah_ledger = env::var("GAH_LEDGER_PATH").ok();
        let saved_gah_events = env::var("GAH_EVENTS_PATH").ok();
        let saved_xdg_state_home = env::var("XDG_STATE_HOME").ok();

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
            config_append: String::new(),
            saved_path,
            saved_gah_config,
            saved_gah_ledger,
            saved_gah_events,
            saved_xdg_state_home,
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

    pub fn with_config_append(mut self, extra_toml: &str) -> Self {
        self.config_append.push_str(extra_toml);
        if !extra_toml.ends_with('\n') {
            self.config_append.push('\n');
        }
        self
    }

    /// Install a custom FakeBackend for `gh` (bypassing the named fixture
    /// loader).  Used by tests that need sequence-based behavior (e.g.
    /// fail → fail → succeed).
    pub fn install_custom_gh(&mut self, fb: &FakeBackend) {
        // Create a new FakeBackend in the harness's own temp dir, then
        // copy the script from `fb` into the harness bin_dir so the
        // subprocess resolves it from PATH.
        let new_fb = FakeBackend::new(self._temp.path(), "gh");
        let src = fb.bin_dir().join("gh");
        if src.exists() {
            let dst = new_fb.bin_dir().join("gh");
            let _ = std::fs::copy(&src, &dst);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&dst) {
                    let mut p = meta.permissions();
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(&dst, p);
                }
            }
        }
        self.fake_gh = Some(new_fb);
    }

    pub fn install_custom_glab(&mut self, fb: &FakeBackend) {
        let new_fb = FakeBackend::new(self._temp.path(), "glab");
        let src = fb.bin_dir().join("glab");
        if src.exists() {
            let dst = new_fb.bin_dir().join("glab");
            let _ = std::fs::copy(&src, &dst);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&dst) {
                    let mut p = meta.permissions();
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(&dst, p);
                }
            }
        }
        self.fake_glab = Some(new_fb);
    }

    pub fn install_custom_worker(&mut self, name: &str, fb: &FakeBackend) {
        let new_fb = FakeBackend::new(self._temp.path(), name);
        let src = fb.bin_dir().join(name);
        if src.exists() {
            let dst = new_fb.bin_dir().join(name);
            let _ = std::fs::copy(&src, &dst);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&dst) {
                    let mut p = meta.permissions();
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(&dst, p);
                }
            }
        }
        self.fake_workers.insert(name.to_string(), new_fb);
    }

    /// Create a branch in the local repo and push it to the bare
    /// origin remote so `git fetch origin <branch>` works.
    /// Adds a dummy change so the branch differs from main (gah won't
    /// review identical branches).
    pub fn create_remote_branch(&self, branch: &str) {
        let r = self.local_repo_dir.to_str().unwrap();
        let _ = std::process::Command::new("git")
            .args(["-C", r, "checkout", "-b", branch])
            .output();
        // Add a distinguishable change so the branch differs from main.
        std::fs::write(
            self.local_repo_dir
                .join(format!("{}.md", branch.replace('/', "-"))),
            format!("# {branch}\n"),
        )
        .ok();
        let _ = std::process::Command::new("git")
            .args(["-C", r, "add", "."])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", r, "commit", "-m", &format!("change on {branch}")])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", r, "push", "-q", "origin", branch])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", r, "checkout", "main"])
            .output();
    }

    fn setup_env(&mut self) {
        // Reject partial fixtures that would silently disappear in
        // production deserialization before any side-effects (config
        // write, env capture, env mutation).  This catches the root
        // cause at harness-setup time and leaves no residual state.
        if let Err(e) = self.ledger.validate_production_schema() {
            panic!("{e}");
        }

        self.write_config();

        let _ = self.ledger.write_to(&self.ledger_path);

        // Host-state isolation: point XDG_STATE_HOME into the harness
        // temp dir so availability, validation, and work-claim state
        // reads go to a per-harness empty directory, not the host's
        // real ~/.local/state/gah/.
        let state_dir = self._temp.path().join("xdg-state");
        fs::create_dir_all(&state_dir).ok();
        env::set_var("XDG_STATE_HOME", state_dir.to_str().unwrap());

        env::set_var("GAH_CONFIG", self.config_path.to_str().unwrap());
        env::set_var("GAH_LEDGER_PATH", self.ledger_path.to_str().unwrap());
        env::set_var("GAH_EVENTS_PATH", self.events_path.to_str().unwrap());

        // PATH: only prepend bin_dir once (not repeatedly on every call).
        let path_env = env::var("PATH");
        let base_path = self.saved_path.as_deref().unwrap_or(match path_env {
            Ok(ref p) => p.as_str(),
            Err(_) => "",
        });
        env::set_var("PATH", format!("{}:{}", self.bin_dir.display(), base_path));
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
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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

    /// Run `gah status --json` against production. This exercises
    /// `status::build_snapshot` → `sync::count_fix_attempts_per_branch`
    /// (and fixed merge counts / MR classification) with the harness
    /// ledger + fake providers. Returns the full JSON snapshot.
    pub fn run_status_json(&mut self) -> Result<serde_json::Value, String> {
        self.setup_env();
        self.install_fakes();
        let out = Command::new(&self.gah_bin)
            .args(["status", "--profile", &self.profile_name, "--json"])
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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
            .map_err(|e| format!("status spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "status exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Status --json prints one JSON object; take the largest {...} span.
        let start = stdout.find('{').ok_or_else(|| {
            format!(
                "no JSON object in status stdout: {}",
                &stdout[..stdout.len().min(200)]
            )
        })?;
        serde_json::from_str(&stdout[start..]).map_err(|e| format!("parse status json: {e}"))
    }

    pub fn run_quota_list_json(&mut self) -> Result<serde_json::Value, String> {
        self.setup_env();
        self.install_fakes();
        let store_path = self.artifacts_dir.join("quota-observations.jsonl");
        fs::write(
            &store_path,
            concat!(
                r#"{"backend":"codex","model":"gpt-5.4-mini","quota_window":"weekly","quota_used_percent":25.0,"quota_remaining_percent":75.0,"quota_reset_at":"2026-07-20T00:00:00Z","observed_at":"2026-07-19T00:00:00Z","usage_source":"codex_status"}"#,
                "\n"
            ),
        )
        .map_err(|e| format!("write quota fixture store: {e}"))?;
        let out = Command::new(&self.gah_bin)
            .args([
                "quota",
                "list",
                "--json",
                "--store-path",
                store_path.to_str().unwrap(),
            ])
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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
            .map_err(|e| format!("quota list spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "quota list exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let start = stdout.find('[').ok_or_else(|| {
            format!(
                "no JSON array in quota list stdout: {}",
                &stdout[..stdout.len().min(200)]
            )
        })?;
        serde_json::from_str(&stdout[start..]).map_err(|e| format!("parse quota list json: {e}"))
    }

    pub fn run_dispatch(&mut self, args: &[&str]) -> Result<CommandResult, String> {
        self.setup_env();
        self.install_fakes();
        let mut cmd = Command::new(&self.gah_bin);
        cmd.args(["dispatch", "--profile", &self.profile_name]);
        cmd.args(args);
        let out = cmd
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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
            .map_err(|e| format!("dispatch spawn failed: {e}"))?;
        Ok(CommandResult {
            exit_code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    pub fn run_ledger_reconcile_json(
        &mut self,
        dry_run: bool,
    ) -> Result<serde_json::Value, String> {
        self.run_ledger_reconcile_json_for_profile(&self.profile_name.clone(), dry_run)
    }

    pub fn run_ledger_reconcile_json_for_profile(
        &mut self,
        profile_name: &str,
        dry_run: bool,
    ) -> Result<serde_json::Value, String> {
        self.setup_env();
        self.install_fakes();
        let mut cmd = Command::new(&self.gah_bin);
        cmd.args(["ledger", "reconcile", "--profile", profile_name, "--json"]);
        if dry_run {
            cmd.arg("--dry-run");
        }
        let out = cmd
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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
            .map_err(|e| format!("reconcile spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "reconcile exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse reconcile json: {e}"))
    }

    pub fn reconciliation_entry_count(&self) -> usize {
        let path = self.artifacts_dir.join("reconciliation.jsonl");
        if !path.exists() {
            return 0;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        text.lines().filter(|l| !l.trim().is_empty()).count()
    }

    pub fn run_report_json(&mut self, group_by: &str) -> Result<serde_json::Value, String> {
        self.setup_env();
        self.install_fakes();
        let out = Command::new(&self.gah_bin)
            .args([
                "report",
                "--json",
                "--group-by",
                group_by,
                "--since",
                "365d",
            ])
            .env(
                "XDG_STATE_HOME",
                self._temp.path().join("xdg-state").to_str().unwrap(),
            )
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
            .map_err(|e| format!("report spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "report exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse report json: {e}"))
    }

    pub fn run_sync_json(&mut self) -> Result<serde_json::Value, String> {
        self.setup_env();
        self.install_fakes();
        let out = Command::new(&self.gah_bin)
            .args(["sync", "--profile", &self.profile_name, "--json"])
            .env("GAH_CONFIG", &self.config_path)
            .env("GAH_LEDGER_PATH", &self.ledger_path)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin_dir.display(),
                    env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .map_err(|e| format!("sync spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "sync exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse sync json: {e}"))
    }

    pub fn github_argv_for_call(&self, call: u32) -> Vec<String> {
        self.fake_gh
            .as_ref()
            .map(|fake| fake.argv_for_call(call))
            .unwrap_or_default()
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
{extra}
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
            extra = self.config_append,
        );
        fs::write(&self.config_path, toml).unwrap();
    }
}

impl Drop for ScenarioHarness {
    fn drop(&mut self) {
        // Restore exact original environment (not just remove).
        restore_var("GAH_CONFIG", &self.saved_gah_config);
        restore_var("GAH_LEDGER_PATH", &self.saved_gah_ledger);
        restore_var("GAH_EVENTS_PATH", &self.saved_gah_events);
        restore_var("XDG_STATE_HOME", &self.saved_xdg_state_home);
        restore_var("PATH", &self.saved_path);
    }
}

fn restore_var(name: &str, saved: &Option<String>) {
    match saved {
        Some(val) => env::set_var(name, val),
        None => env::remove_var(name),
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

/// Build a minimal GitHub PR JSON payload suitable for the real `gh pr list --json`
/// deserialization path.  All fields marked `#[serde(default)]` in `GithubPr` can
/// be omitted; this helper fills in just enough for the MR to be picked up
/// (branch starts with `gah/`) and classified correctly.
pub fn github_pr_json(params: GithubPrParams) -> serde_json::Value {
    let mut labels = Vec::new();
    for name in &params.labels {
        labels.push(serde_json::json!({"name": name}));
    }
    let checks: serde_json::Value = match params.ci_conclusion {
        Some(ref conclusion) => serde_json::json!([{"conclusion": conclusion}]),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "title": params.title,
        "headRefName": params.branch,
        "url": params.url.unwrap_or("https://github.com/owner/repo/pull/1".into()),
        "labels": labels,
        "number": params.number.unwrap_or(1),
        "state": params.state.as_deref().unwrap_or("OPEN"),
        "isDraft": params.draft.unwrap_or(true),
        "mergeStateStatus": "CLEAN",
        "mergedAt": params.merged_at,
        "updatedAt": params.updated_at.unwrap_or("2026-07-01T00:00:00Z".into()),
        "statusCheckRollup": checks,
    })
}

pub struct GithubPrParams {
    pub title: String,
    pub branch: String,
    pub labels: Vec<String>,
    pub ci_conclusion: Option<String>,
    pub state: Option<String>,
    pub url: Option<String>,
    pub number: Option<i64>,
    pub draft: Option<bool>,
    pub merged_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Build a minimal GitLab MR JSON payload suitable for `glab mr list --output json`
/// deserialization.
pub fn gitlab_mr_json(params: GitlabMrParams) -> serde_json::Value {
    let pipeline = match params.pipeline_status {
        Some(ref status) => serde_json::json!({"status": status}),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "title": params.title,
        "source_branch": params.branch,
        "target_branch": "main",
        "web_url": params.url.unwrap_or("https://gitlab.example.com/group/repo/-/merge_requests/1".into()),
        "labels": params.labels,
        "iid": params.iid.unwrap_or(1),
        "state": params.state.as_deref().unwrap_or("opened"),
        "draft": params.draft.unwrap_or(true),
        "detailed_merge_status": "mergeable",
        "merged_at": params.merged_at,
        "updated_at": params.updated_at.unwrap_or("2026-07-01T00:00:00Z".into()),
        "head_pipeline": pipeline,
    })
}

pub struct GitlabMrParams {
    pub title: String,
    pub branch: String,
    pub labels: Vec<String>,
    pub pipeline_status: Option<String>,
    pub url: Option<String>,
    pub iid: Option<i64>,
    pub state: Option<String>,
    pub draft: Option<bool>,
    pub merged_at: Option<String>,
    pub updated_at: Option<String>,
}

fn load_github_fixture(name: &str) -> Scenario {
    match name {
        "empty" => Scenario::success().with_stdout("[]"),
        "malformed" => Scenario::success().with_stdout("not json"),
        "non_zero_exit" => Scenario::failure(1),
        // Real July 2026 incident: statusCheckRollup: null was the exact payload
        // that historically caused deserialization failure.  The fix is already
        // in place (GithubPr.status_check_rollup is Option<Vec<GithubCheck>>,
        // so null → None), but this controller-level regression proves the full
        // loop handles it without spinning.
        "prs_closed_null_rollup" => Scenario::success().with_stdout(
            serde_json::to_string(&vec![github_pr_json(GithubPrParams {
                title: "Draft: TICKET-001 test".into(),
                branch: "gah/test-1".into(),
                labels: vec![],
                ci_conclusion: None,
                state: None,
                url: None,
                number: None,
                draft: None,
                merged_at: None,
                updated_at: None,
            })])
            .unwrap(),
        ),
        // One open gah/ PR with no labels → NEEDS_REVIEW
        "one_pr_needs_review" => Scenario::success().with_stdout(
            serde_json::to_string(&vec![github_pr_json(GithubPrParams {
                title: "Draft: TICKET-001 Add feature".into(),
                branch: "gah/feature-1".into(),
                labels: vec![],
                ci_conclusion: Some("SUCCESS".into()),
                state: None,
                url: None,
                number: None,
                draft: None,
                merged_at: None,
                updated_at: None,
            })])
            .unwrap(),
        ),
        // PR with gah-needs-fix label → NEEDS_FIX
        "one_pr_needs_fix" => Scenario::success().with_stdout(
            serde_json::to_string(&vec![github_pr_json(GithubPrParams {
                title: "Draft: TICKET-001 Fix bug".into(),
                branch: "gah/fix-1".into(),
                labels: vec!["gah-needs-fix".into()],
                ci_conclusion: Some("SUCCESS".into()),
                state: None,
                url: None,
                number: None,
                draft: None,
                merged_at: None,
                updated_at: None,
            })])
            .unwrap(),
        ),
        // PR with gah-ready-for-human label → READY_FOR_HUMAN
        "one_pr_ready_for_human" => Scenario::success().with_stdout(
            serde_json::to_string(&vec![github_pr_json(GithubPrParams {
                title: "Draft: TICKET-001 Ready".into(),
                branch: "gah/ready-1".into(),
                labels: vec!["gah-ready-for-human".into()],
                ci_conclusion: Some("SUCCESS".into()),
                state: None,
                url: None,
                number: None,
                draft: None,
                merged_at: None,
                updated_at: None,
            })])
            .unwrap(),
        ),
        // PR with CI FAILURE → CI_FAILED
        "one_pr_ci_failed" => Scenario::success().with_stdout(
            serde_json::to_string(&vec![github_pr_json(GithubPrParams {
                title: "Draft: TICKET-001 CI broken".into(),
                branch: "gah/ci-fail-1".into(),
                labels: vec![],
                ci_conclusion: Some("FAILURE".into()),
                state: None,
                url: None,
                number: None,
                draft: None,
                merged_at: None,
                updated_at: None,
            })])
            .unwrap(),
        ),
        // Manual `gah dispatch --mode fix --mr <id>` path for provider/manual fixture.
        // The resolved MR is assumed to target this branch, so resolver + repair
        // context can reuse it without passing --existing-branch.
        "manual_fix_needs_fix" => Scenario::success().with_stdout(
            serde_json::to_string(&github_pr_json(GithubPrParams {
                title: "Draft: TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                ci_conclusion: Some("SUCCESS".into()),
                state: Some("OPEN".into()),
                url: None,
                number: Some(269),
                draft: None,
                merged_at: None,
                updated_at: None,
            }))
            .unwrap(),
        ),
        "manual_fix_needs_fix_closed" => Scenario::success().with_stdout(
            serde_json::to_string(&github_pr_json(GithubPrParams {
                title: "Draft: TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                ci_conclusion: Some("SUCCESS".into()),
                state: Some("CLOSED".into()),
                url: None,
                number: Some(269),
                draft: None,
                merged_at: None,
                updated_at: None,
            }))
            .unwrap(),
        ),
        "manual_fix_needs_fix_merged" => Scenario::success().with_stdout(
            serde_json::to_string(&github_pr_json(GithubPrParams {
                title: "Draft: TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                ci_conclusion: Some("SUCCESS".into()),
                state: Some("MERGED".into()),
                url: None,
                number: Some(269),
                draft: None,
                merged_at: Some("2026-07-01T00:00:00Z".into()),
                updated_at: None,
            }))
            .unwrap(),
        ),
        _ => Scenario::failure(127).with_stderr(format!("unknown github scenario: {name}")),
    }
}

fn load_gitlab_fixture(name: &str) -> Scenario {
    match name {
        "empty" => Scenario::success().with_stdout("[]"),
        "malformed" => Scenario::success().with_stdout("not json"),
        "non_zero_exit" => Scenario::failure(1),
        // Manual `gah dispatch --mode fix --mr <id>` path for GitLab provider.
        "manual_fix_needs_fix" => Scenario::success().with_stdout(
            serde_json::to_string(&gitlab_mr_json(GitlabMrParams {
                title: "TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                pipeline_status: None,
                url: None,
                iid: Some(269),
                state: Some("opened".into()),
                draft: Some(false),
                merged_at: None,
                updated_at: None,
            }))
            .unwrap(),
        ),
        "manual_fix_needs_fix_closed" => Scenario::success().with_stdout(
            serde_json::to_string(&gitlab_mr_json(GitlabMrParams {
                title: "TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                pipeline_status: None,
                url: None,
                iid: Some(269),
                state: Some("closed".into()),
                draft: Some(false),
                merged_at: None,
                updated_at: None,
            }))
            .unwrap(),
        ),
        "manual_fix_needs_fix_merged" => Scenario::success().with_stdout(
            serde_json::to_string(&gitlab_mr_json(GitlabMrParams {
                title: "TICKET-269 Fix race".into(),
                branch: "gah/fix-needs-fix".into(),
                labels: vec!["gah-needs-fix".into()],
                pipeline_status: None,
                url: None,
                iid: Some(269),
                state: Some("merged".into()),
                draft: Some(false),
                merged_at: Some("2026-07-01T00:00:00Z".into()),
                updated_at: None,
            }))
            .unwrap(),
        ),
        _ => Scenario::failure(127).with_stderr(format!("unknown gitlab scenario: {name}")),
    }
}

fn load_worker_fixture(name: &str) -> Scenario {
    match name {
        "success" => Scenario::success().with_stdout("work complete\n"),
        "failure" => Scenario::failure(1).with_stderr("worker failed"),
        "empty_success" => Scenario::success(),
        "invalid_output" => Scenario::success().with_stdout("unexpected output"),
        "review_approve" => Scenario::success().with_stdout(
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        ),
        "hang" => Scenario {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            delay_ms: 300_000,
        },
        _ => Scenario::failure(127).with_stderr(format!("unknown worker scenario: {name}")),
    }
}
