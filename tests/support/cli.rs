#![allow(dead_code)]

#[cfg(unix)]
use std::ffi::CString;
use std::fs;
use std::ops::{Deref, DerefMut};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[cfg(unix)]
const TEST_INTEGRATION_MIN_DISPATCH_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Keep integration-test repositories and child-process temporary files on
/// the same filesystem as Cargo's build output. Some development hosts mount
/// `/tmp` as a small tmpfs; production dispatch capacity checks must remain
/// strict without making the test suite depend on that unrelated filesystem's
/// current free-space level.
pub fn test_temp_root() -> PathBuf {
    let fallback_root = std::env::temp_dir();
    let default_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| path.parent()?.parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| fallback_root.clone());
    let candidates = [
        default_root.clone(),
        PathBuf::from("/dev/shm"),
        fallback_root.clone(),
    ];
    let root = candidates
        .into_iter()
        .find(|candidate| has_minimum_dispatch_space(candidate))
        .unwrap_or(default_root)
        .join("gah-integration-test-tmp");
    fs::create_dir_all(&root).unwrap();
    root
}

#[cfg(unix)]
fn has_minimum_dispatch_space(path: &Path) -> bool {
    match free_space_bytes(path) {
        Some(available) => available >= TEST_INTEGRATION_MIN_DISPATCH_BYTES,
        None => false,
    }
}

#[cfg(unix)]
fn free_space_bytes(path: &Path) -> Option<u64> {
    let filesystem_path = path.ancestors().find(|ancestor| ancestor.exists())?;
    let path_c = CString::new(filesystem_path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path_c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(not(unix))]
fn has_minimum_dispatch_space(_path: &Path) -> bool {
    true
}

pub fn test_tempdir() -> TempDir {
    tempfile::Builder::new()
        .prefix("gah-test-")
        .tempdir_in(test_temp_root())
        .unwrap()
}

/// Retains a command's isolated filesystem environment until the command is
/// finished, then removes it with the fixture. This matters for asynchronously
/// spawned children: dropping the temporary directory immediately after
/// `spawn()` would race the child while leaving anything it recreated behind.
pub struct IsolatedCommand<C> {
    command: C,
    state: TempDir,
}

impl<C> IsolatedCommand<C> {
    pub fn state_path(&self) -> &Path {
        self.state.path()
    }
}

impl<C> Deref for IsolatedCommand<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        &self.command
    }
}

impl<C> DerefMut for IsolatedCommand<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.command
    }
}

pub fn isolate_command<C>(
    mut command: C,
    set_env: impl FnOnce(&mut C, &Path),
) -> IsolatedCommand<C> {
    let state = test_tempdir();
    set_env(&mut command, state.path());
    IsolatedCommand { command, state }
}

pub trait TestCommandEnvironment {
    fn env_path(&mut self, key: &str, value: &Path);
}

impl TestCommandEnvironment for assert_cmd::Command {
    fn env_path(&mut self, key: &str, value: &Path) {
        self.env(key, value);
    }
}

impl TestCommandEnvironment for std::process::Command {
    fn env_path(&mut self, key: &str, value: &Path) {
        self.env(key, value);
    }
}

/// Isolate all process-global GAH state used by CLI integration children,
/// including the node-wide capacity lease registry. CLI integration tests
/// may run under the real systemd loop, which sets XDG_STATE_HOME to the
/// operator's persistent state directory; never let fake profiles and work
/// claims leak into (or inherit from) that state.
pub fn isolate_gah_command<C: TestCommandEnvironment>(command: C) -> IsolatedCommand<C> {
    isolate_command(command, |command, root| {
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        command.env_path("XDG_STATE_HOME", &root.join("xdg-state"));
        command.env_path("XDG_RUNTIME_DIR", &root.join("xdg-runtime"));
        command.env_path("GAH_AVAILABILITY_PATH", &root.join("availability.json"));
        command.env_path(
            "GAH_VALIDATION_CHECK_PATH",
            &root.join("validation-check.json"),
        );
        command.env_path("TMPDIR", &tmp);
    })
}

pub fn gah_command() -> IsolatedCommand<assert_cmd::Command> {
    isolate_gah_command(assert_cmd::Command::cargo_bin("gah").unwrap())
}

pub fn gah_process_command() -> IsolatedCommand<std::process::Command> {
    let command = std::process::Command::new(
        std::env::var("CARGO_BIN_EXE_gah").unwrap_or_else(|_| "target/debug/gah".into()),
    );
    isolate_gah_command(command)
}

pub fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

pub fn write_fake_binary(dir: &Path, name: &str, body: &str) {
    write_executable(&dir.join(name), body);
}
