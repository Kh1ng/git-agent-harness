//! Issue #116: best-effort process-tree resource sampling for supervised
//! backend attempts. Split from `process.rs` to keep that file under the
//! source-size guard. The sampler runs alongside the supervision loop
//! (idle watch or review loop), walking the whole backend process tree by
//! PPID so setsid escapees and the root agent PID both count.

#[cfg(target_os = "linux")]
use super::process::{linux_descendants, linux_process_snapshot, ProcessIdentity};
#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// Issue #116: accumulated process-tree resource totals for one supervised
/// backend attempt. CPU time is delta-summed per process so a process that
/// exits mid-attempt still contributes its full consumption; peak RSS is the
/// maximum tree-wide resident total observed at any sample.
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct ResourceAccumulator {
    observed_any_sample: bool,
    last_ticks_by_process: HashMap<ProcessIdentity, u64>,
    cpu_ticks_total: u64,
    peak_rss_bytes: u64,
}

#[cfg(target_os = "linux")]
fn linux_clock_ticks_per_second() -> f64 {
    // _SC_CLK_TCK is fixed at boot on Linux; the default of 100 is the
    // fallback if sysconf is somehow unavailable.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as f64
    } else {
        100.0
    }
}

#[cfg(target_os = "linux")]
fn linux_rss_bytes(pid: u32, page_size: f64) -> Option<u64> {
    let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some((resident_pages as f64 * page_size) as u64)
}

/// One sample of the whole backend process tree (root included — the ticket
/// explicitly requires the complete tree, not only the short-lived wrapper
/// PID). Returns `(identity, cpu_ticks, rss_bytes)` for every live member.
#[cfg(target_os = "linux")]
fn linux_tree_usage_sample(
    root_pid: u32,
    page_size: f64,
) -> Vec<(ProcessIdentity, u64, Option<u64>)> {
    let mut members = Vec::new();
    if let Some(root) = linux_process_snapshot(root_pid) {
        members.push(root.identity);
    }
    for identity in linux_descendants(root_pid) {
        members.push(identity);
    }
    members
        .into_iter()
        .filter_map(|identity| {
            let stat = fs::read_to_string(format!("/proc/{}/stat", identity.pid)).ok()?;
            let fields = stat[stat.rfind(") ")? + 2..]
                .split_whitespace()
                .collect::<Vec<_>>();
            let user_ticks = fields.get(11)?.parse::<u64>().ok()?;
            let system_ticks = fields.get(12)?.parse::<u64>().ok()?;
            Some((
                identity,
                user_ticks.saturating_add(system_ticks),
                linux_rss_bytes(identity.pid, page_size),
            ))
        })
        .collect()
}

/// Issue #116: best-effort sampler thread for one backend attempt. Samples
/// every 250 ms until stopped; never blocks or crashes the supervised run.
/// Started right after spawn so even a fast-exiting backend has a chance to
/// be observed; a tree that dies before the first completed sample records
/// an explicit unknown, never zero.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_resource_sampler(
    root_pid: u32,
) -> (
    Arc<AtomicBool>,
    Arc<Mutex<ResourceAccumulator>>,
    thread::JoinHandle<()>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let accumulator = Arc::new(Mutex::new(ResourceAccumulator::default()));
    let stop_for_thread = Arc::clone(&stop);
    let accumulator_for_thread = Arc::clone(&accumulator);
    let handle = thread::spawn(move || {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as f64;
        while !stop_for_thread.load(Ordering::SeqCst) {
            let sample = linux_tree_usage_sample(root_pid, page_size);
            if let Ok(mut accum) = accumulator_for_thread.lock() {
                accum.observed_any_sample = true;
                let mut tree_rss_total = 0_u64;
                for (identity, cpu_ticks, rss_bytes) in sample {
                    let last = accum
                        .last_ticks_by_process
                        .insert(identity, cpu_ticks)
                        .unwrap_or(0);
                    accum.cpu_ticks_total += cpu_ticks.saturating_sub(last);
                    tree_rss_total += rss_bytes.unwrap_or(0);
                }
                if tree_rss_total > accum.peak_rss_bytes {
                    accum.peak_rss_bytes = tree_rss_total;
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
    (stop, accumulator, handle)
}

#[cfg(target_os = "linux")]
pub(crate) fn finish_resource_sampler(
    stop: Arc<AtomicBool>,
    accumulator: Arc<Mutex<ResourceAccumulator>>,
    handle: thread::JoinHandle<()>,
) -> crate::ledger::AttemptResourceUsage {
    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let accum = accumulator.lock().map(|guard| guard.clone());
    let Ok(accum) = accum else {
        return crate::ledger::AttemptResourceUsage {
            cpu_time_seconds: Some(crate::ledger::ResourceMetric::unknown(
                "resource accumulator lock poisoned",
            )),
            peak_rss_bytes: Some(crate::ledger::ResourceMetric::unknown(
                "resource accumulator lock poisoned",
            )),
        };
    };
    if !accum.observed_any_sample {
        return crate::ledger::AttemptResourceUsage {
            cpu_time_seconds: Some(crate::ledger::ResourceMetric::unknown(
                "backend process tree exited before the first resource sample",
            )),
            peak_rss_bytes: Some(crate::ledger::ResourceMetric::unknown(
                "backend process tree exited before the first resource sample",
            )),
        };
    }
    let ticks_per_second = linux_clock_ticks_per_second();
    crate::ledger::AttemptResourceUsage {
        cpu_time_seconds: Some(crate::ledger::ResourceMetric::measured(
            accum.cpu_ticks_total as f64 / ticks_per_second,
        )),
        peak_rss_bytes: Some(crate::ledger::ResourceMetric::measured(
            accum.peak_rss_bytes as f64,
        )),
    }
}

/// Non-Linux platforms cannot measure backend process trees (/proc is the
/// only supported mechanism), so the attempt records explicit unsupported
/// provenance — never zero, never silently absent.
#[cfg(not(target_os = "linux"))]
pub(crate) fn finish_resource_sampler_none() -> crate::ledger::AttemptResourceUsage {
    crate::ledger::AttemptResourceUsage {
        cpu_time_seconds: Some(crate::ledger::ResourceMetric::unsupported(
            "process-tree resource sampling is only implemented on Linux",
        )),
        peak_rss_bytes: Some(crate::ledger::ResourceMetric::unsupported(
            "process-tree resource sampling is only implemented on Linux",
        )),
    }
}
