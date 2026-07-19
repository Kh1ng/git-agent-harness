//! Ownership contract for the native recurring controller.
//!
//! A foreground loop may be launched manually, but it must die with that
//! launcher. Systemd is the only supported detached owner and additionally
//! supplies the cgroup boundary that contains every backend descendant.

use anyhow::{bail, Context, Result};

fn validate_live_parent(parent: libc::pid_t) -> Result<()> {
    if parent == 1 {
        bail!(
            "refusing to start an orphaned recurring loop (parent PID is 1); use `systemctl --user start gah-loop@<profile>` for detached operation"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn arm_parent_death_signal() -> Result<()> {
    let parent_before = unsafe { libc::getppid() };
    validate_live_parent(parent_before)?;

    let result = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    if result == -1 {
        return Err(std::io::Error::last_os_error())
            .context("arming recurring-loop parent-death signal");
    }

    // PR_SET_PDEATHSIG is race-free only when the parent identity is checked
    // again after arming it. If the launcher exited between getppid and prctl,
    // fail before acquiring the profile lock or spawning any worker.
    let parent_after = unsafe { libc::getppid() };
    if parent_after != parent_before || parent_after == 1 {
        bail!(
            "recurring-loop launcher exited during startup; use the systemd user unit for detached operation"
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn arm_parent_death_signal() -> Result<()> {
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const TEST_NAME: &str =
        "controller::ownership::tests::armed_process_exits_when_its_launcher_dies";
    const ROLE_ENV: &str = "GAH_PARENT_DEATH_TEST_ROLE";
    const STATE_ENV: &str = "GAH_PARENT_DEATH_TEST_STATE";

    fn process_exists(pid: i32) -> bool {
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[test]
    fn pid_one_is_never_accepted_as_a_recurring_loop_owner() {
        let error = validate_live_parent(1).unwrap_err().to_string();
        assert!(error.contains("orphaned recurring loop"));
        assert!(error.contains("systemctl --user start"));
    }

    // The launcher role must deliberately exit without waiting for its child:
    // that parent disappearance is the condition this subprocess test exercises.
    #[allow(clippy::zombie_processes)]
    #[test]
    fn armed_process_exits_when_its_launcher_dies() {
        let role = std::env::var(ROLE_ENV).ok();
        let state = std::env::var_os(STATE_ENV).map(std::path::PathBuf::from);

        if role.as_deref() == Some("child") {
            crate::runner::install_shutdown_handler().unwrap();
            arm_parent_death_signal().unwrap();
            fs::write(state.unwrap().join("ready"), std::process::id().to_string()).unwrap();
            while !crate::runner::shutdown_requested() {
                thread::sleep(Duration::from_millis(10));
            }
            return;
        }

        if role.as_deref() == Some("launcher") {
            let state = state.unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(ROLE_ENV, "child")
                .env(STATE_ENV, &state)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !state.join("ready").exists() {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "child exited before arming"
                );
                assert!(
                    Instant::now() < deadline,
                    "child did not arm parent-death signal"
                );
                thread::sleep(Duration::from_millis(10));
            }
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(ROLE_ENV, "launcher")
            .env(STATE_ENV, tmp.path())
            .status()
            .unwrap();
        assert!(status.success());

        let pid = fs::read_to_string(tmp.path().join("ready"))
            .unwrap()
            .parse::<i32>()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_exists(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !process_exists(pid),
            "armed child {pid} survived launcher death"
        );
    }
}
