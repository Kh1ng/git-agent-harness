#[cfg(target_os = "linux")]
use super::{linux_process_snapshot, ProcessIdentity};
use crate::ledger::ProcessResources;
#[cfg(target_os = "linux")]
use std::collections::{HashMap, HashSet};

/// Retains observed maxima across process exits, PID reuse and every supervisor
/// termination path. No arguments, environment or process names leave procfs.
pub(crate) struct ResourceSampler {
    observation: ProcessResources,
    #[cfg(target_os = "linux")]
    root_pid: u32,
    #[cfg(target_os = "linux")]
    last_sample: Option<std::time::Instant>,
    #[cfg(target_os = "linux")]
    cpu_ticks: HashMap<ProcessIdentity, u64>,
    #[cfg(target_os = "linux")]
    clock_ticks: f64,
    #[cfg(target_os = "linux")]
    page_bytes: u64,
}

impl ResourceSampler {
    pub(crate) fn new(_root_pid: u32) -> Self {
        Self {
            observation: ProcessResources::unknown(if cfg!(target_os = "linux") {
                "process_exited_before_observation_or_procfs_unreadable"
            } else {
                "unsupported_platform"
            }),
            #[cfg(target_os = "linux")]
            root_pid: _root_pid,
            #[cfg(target_os = "linux")]
            last_sample: None,
            #[cfg(target_os = "linux")]
            cpu_ticks: HashMap::new(),
            #[cfg(target_os = "linux")]
            clock_ticks: unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64,
            #[cfg(target_os = "linux")]
            page_bytes: unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64,
        }
    }

    pub(crate) fn sample(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // Reviewers poll every 25ms; a procfs tree scan need not follow that cadence.
            if self
                .last_sample
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(250))
            {
                return;
            }
            self.last_sample = Some(std::time::Instant::now());
            if self.clock_ticks <= 0.0 || self.page_bytes == 0 {
                self.observation = ProcessResources::unknown("platform_units_unavailable");
                return;
            }
            let Ok(directory) = std::fs::read_dir("/proc") else {
                if self.cpu_ticks.is_empty() {
                    self.observation = ProcessResources::unknown("procfs_unreadable");
                }
                return;
            };
            let snapshots = directory
                .flatten()
                .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
                .filter_map(linux_process_snapshot)
                .collect::<Vec<_>>();
            let mut members = snapshots
                .iter()
                .filter(|snapshot| {
                    snapshot.identity.pid == self.root_pid
                        || snapshot.group_pid == self.root_pid
                        || self.cpu_ticks.contains_key(&snapshot.identity)
                })
                .map(|snapshot| snapshot.identity.pid)
                .collect::<HashSet<_>>();
            loop {
                let mut changed = false;
                for snapshot in &snapshots {
                    if members.contains(&snapshot.parent_pid)
                        && members.insert(snapshot.identity.pid)
                    {
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let mut resident_pages = 0u64;
            let mut observed = false;
            for snapshot in snapshots
                .iter()
                .filter(|snapshot| members.contains(&snapshot.identity.pid))
            {
                // An unobserved zombie's zero RSS cannot establish a memory measurement.
                if snapshot.zombie && !self.cpu_ticks.contains_key(&snapshot.identity) {
                    continue;
                }
                observed = true;
                self.cpu_ticks
                    .entry(snapshot.identity)
                    .and_modify(|ticks| *ticks = (*ticks).max(snapshot.cpu_ticks))
                    .or_insert(snapshot.cpu_ticks);
                resident_pages = resident_pages.saturating_add(snapshot.rss_pages);
            }
            if observed {
                self.observation.cpu_seconds = Some(
                    self.cpu_ticks
                        .values()
                        .map(|ticks| *ticks as f64)
                        .sum::<f64>()
                        / self.clock_ticks,
                );
                self.observation.peak_rss_bytes = Some(
                    self.observation
                        .peak_rss_bytes
                        .unwrap_or(0)
                        .max(resident_pages.saturating_mul(self.page_bytes)),
                );
                self.observation.source = "linux_procfs_sampled_lower_bound".into();
                self.observation.unknown_reason = None;
            }
        }
    }

    pub(crate) fn finish(self) -> ProcessResources {
        self.observation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_process_is_unknown_not_zero() {
        let mut sampler = ResourceSampler::new(u32::MAX);
        sampler.sample();
        let observation = sampler.finish();
        assert_eq!(observation.cpu_seconds, None);
        assert_eq!(observation.peak_rss_bytes, None);
        assert!(observation.unknown_reason.is_some());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod workload_tests {
    use super::super::spawn_with_idle_watch_with_shutdown;
    use std::process::Command;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    #[test]
    fn child_workload_survives_success_failure_timeout_and_cancellation_accounting() {
        for outcome in ["success", "failure", "timeout", "cancelled"] {
            let dir = tempfile::tempdir().unwrap();
            let fixture = dir.path().join("workload.py");
            std::fs::write(
                &fixture,
                r#"import os, time
pid = os.fork()
if pid == 0:
    os.setsid()
    allocation = bytearray(40 * 1024 * 1024)
    end = time.monotonic() + 1.7
    while time.monotonic() < end:
        allocation[0] = (allocation[0] + 1) % 255
    os._exit(0)
os.waitpid(pid, 0)
"#,
            )
            .unwrap();
            let log = dir.path().join("backend.log");
            std::fs::write(&log, "").unwrap();
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg(if outcome == "failure" {
                    r#"python3 "$1"; exit 7"#
                } else {
                    r#"python3 "$1""#
                })
                .arg("fixture")
                .arg(&fixture);
            if outcome == "timeout" {
                command.env(super::super::HARD_TIMEOUT_ENV, "1");
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = Arc::clone(&cancelled);
            let thread = (outcome == "cancelled").then(|| {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(900));
                    signal.store(true, Ordering::SeqCst);
                })
            });
            let (exit, _, resources) = spawn_with_idle_watch_with_shutdown(
                command,
                &log,
                dir.path(),
                10,
                "resource fixture",
                &cancelled,
                true,
            )
            .unwrap();
            if let Some(thread) = thread {
                thread.join().unwrap();
            }
            assert_eq!(
                exit,
                match outcome {
                    "success" => 0,
                    "failure" => 7,
                    "timeout" => -1,
                    _ => -2,
                }
            );
            let cpu = resources.cpu_seconds.expect(outcome);
            let rss = resources.peak_rss_bytes.expect(outcome);
            assert!(cpu > 0.005 && cpu < 10.0, "{outcome}: {resources:?}");
            assert!(
                (32 * 1024 * 1024..512 * 1024 * 1024).contains(&rss),
                "{outcome}: {resources:?}"
            );
            assert_eq!(resources.source, "linux_procfs_sampled_lower_bound");
        }
    }
}
