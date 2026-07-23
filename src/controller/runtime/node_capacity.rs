use crate::controller::NextAction;
use fs2::FileExt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const GIB: u64 = 1024 * 1024 * 1024;
const MIN_MEMORY_RESERVE: u64 = 2 * GIB;
const MIN_CRITICAL_MEMORY: u64 = 512 * 1024 * 1024;

/// A cheap, point-in-time view of pressure on the node running GAH.
///
/// The controller deliberately reads kernel interfaces directly instead of
/// adding a monitoring dependency. If a platform does not expose one of the
/// Linux pressure files, the remaining signals still provide a useful bound.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(debug_assertions, derive(serde::Deserialize))]
pub(crate) struct NodePressure {
    pub(crate) memory_total_bytes: u64,
    pub(crate) memory_available_bytes: u64,
    pub(crate) logical_cpus: usize,
    pub(crate) load_one: f64,
    pub(crate) memory_full_psi_avg10: Option<f64>,
    pub(crate) cpu_some_psi_avg10: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WorkerReservation {
    memory_bytes: u64,
    cpu_units: f64,
}

impl std::ops::AddAssign for WorkerReservation {
    fn add_assign(&mut self, other: Self) {
        self.memory_bytes = self.memory_bytes.saturating_add(other.memory_bytes);
        self.cpu_units += other.cpu_units;
    }
}

impl std::ops::SubAssign for WorkerReservation {
    fn sub_assign(&mut self, other: Self) {
        self.memory_bytes = self.memory_bytes.saturating_sub(other.memory_bytes);
        self.cpu_units = (self.cpu_units - other.cpu_units).max(0.0);
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Admission {
    Admit(WorkerReservation),
    Defer(String),
}

#[derive(Debug)]
pub(crate) struct NodeCapacityLease {
    path: PathBuf,
    _file: std::fs::File,
}

#[derive(Debug)]
pub(crate) enum LiveAdmission {
    Admit(NodeCapacityLease),
    Defer(String),
}

/// Reserve according to the kind of work, rather than treating a reviewer
/// and a compiler-heavy implementation as equal-sized workers. These are
/// admission reservations, not hard limits: live MemAvailable, load, and PSI
/// are sampled again before every subsequent worker and refill.
fn reservation_for(action: &NextAction) -> WorkerReservation {
    match action {
        NextAction::DispatchTicket { .. }
        | NextAction::FixMr { .. }
        | NextAction::Retry { .. }
        | NextAction::Escalate { .. } => WorkerReservation {
            memory_bytes: 4 * GIB,
            cpu_units: 2.0,
        },
        NextAction::DecomposeIssue { .. } => WorkerReservation {
            memory_bytes: 2 * GIB,
            cpu_units: 1.0,
        },
        NextAction::ReviewMr { .. } => WorkerReservation {
            memory_bytes: GIB,
            cpu_units: 0.5,
        },
        NextAction::MarkReadyForReview { .. }
        | NextAction::MergeMr { .. }
        | NextAction::ReconcilePmParent { .. } => WorkerReservation {
            memory_bytes: 256 * 1024 * 1024,
            cpu_units: 0.25,
        },
        NextAction::WaitUntil { .. }
        | NextAction::HumanRequired { .. }
        | NextAction::NoOp { .. } => WorkerReservation::default(),
    }
}

pub(crate) fn admission_for(
    action: &NextAction,
    active_workers: usize,
    committed: WorkerReservation,
    pressure: NodePressure,
) -> Admission {
    let requested = reservation_for(action);
    let memory_reserve = MIN_MEMORY_RESERVE.max(pressure.memory_total_bytes / 6);
    let critical_memory = MIN_CRITICAL_MEMORY.max(pressure.memory_total_bytes / 32);

    if pressure.memory_available_bytes <= critical_memory {
        return Admission::Defer(format!(
            "node memory is critical ({} MiB available)",
            pressure.memory_available_bytes / (1024 * 1024)
        ));
    }

    // Bound headroom by both independent views of the node: live availability
    // captures materialized usage and total-minus-commitments captures workers
    // that have not reached peak yet. Taking the minimum avoids counting the
    // same active worker twice while still preventing a launch burst from
    // spending one idle MemAvailable sample repeatedly.
    let uncommitted_capacity = pressure
        .memory_total_bytes
        .saturating_sub(committed.memory_bytes);
    let effective_available = pressure.memory_available_bytes.min(uncommitted_capacity);
    let projected_available = effective_available.saturating_sub(requested.memory_bytes);
    if projected_available < memory_reserve {
        return Admission::Defer(format!(
            "node memory reserve would be crossed ({} MiB available, {} MiB reserved for active/new workers, {} MiB safety floor)",
            pressure.memory_available_bytes / (1024 * 1024),
            (committed.memory_bytes + requested.memory_bytes) / (1024 * 1024),
            memory_reserve / (1024 * 1024)
        ));
    }

    if pressure.memory_full_psi_avg10.unwrap_or(0.0) >= 1.0 {
        return Admission::Defer(format!(
            "node memory PSI is saturated ({:.2}% full avg10)",
            pressure.memory_full_psi_avg10.unwrap_or(0.0)
        ));
    }

    // Always allow one worker on a node that has safe memory. This keeps a
    // busy shared machine making progress, while additional workers must fit
    // below both the CPU headroom and PSI thresholds.
    if active_workers > 0 {
        let cpu_ceiling = (pressure.logical_cpus.max(1) as f64 * 0.90).max(1.0);
        // Load already includes CPU consumed by active GAH workers. Compare
        // the larger of live load and projected commitments to the ceiling,
        // rather than adding both representations of the same work.
        let projected_cpu = pressure.load_one.max(committed.cpu_units) + requested.cpu_units;
        if projected_cpu > cpu_ceiling {
            return Admission::Defer(format!(
                "node CPU reserve would be crossed (max(load {:.2}, committed {:.2}) + {:.2} requested > {:.2})",
                pressure.load_one,
                committed.cpu_units,
                requested.cpu_units,
                cpu_ceiling
            ));
        }
        if pressure.cpu_some_psi_avg10.unwrap_or(0.0) >= 50.0 {
            return Admission::Defer(format!(
                "node CPU PSI is saturated ({:.2}% some avg10)",
                pressure.cpu_some_psi_avg10.unwrap_or(0.0)
            ));
        }
    }

    Admission::Admit(requested)
}

/// Atomically account for projected worker demand across every GAH profile on
/// this host. A lock is held on each lease file for the worker lifetime. If a
/// controller is killed, the kernel releases that lock and the next admission
/// sweep removes the stale file, so crashed managers cannot strand capacity.
pub(crate) fn try_acquire(
    action: &NextAction,
    local_active_workers: usize,
) -> std::io::Result<LiveAdmission> {
    acquire_in_dir(&capacity_dir(), action, sample()?, local_active_workers)
}

fn acquire_in_dir(
    dir: &Path,
    action: &NextAction,
    pressure: NodePressure,
    local_active_workers: usize,
) -> std::io::Result<LiveAdmission> {
    std::fs::create_dir_all(dir)?;
    let registry_lock = open_registry_lock(dir)?;
    registry_lock.lock_exclusive()?;

    let mut committed = WorkerReservation::default();
    let mut active_workers = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("lease") {
            continue;
        }
        let Some(mut file) = open_existing_lease(&path)? else {
            // A previous process may have completed between read_dir and
            // open. That is a normal release race, not an integrity failure.
            continue;
        };
        match file.try_lock_exclusive() {
            Ok(()) => {
                // No process holds this lease anymore.
                drop(file);
                let _ = std::fs::remove_file(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let mut encoded = String::new();
                file.read_to_string(&mut encoded)?;
                let reservation = decode_reservation(&encoded)?;
                committed += reservation;
                active_workers += 1;
            }
            Err(error) => return Err(error),
        }
    }

    let admission_active_workers = active_workers.max(local_active_workers);
    let reservation = match admission_for(action, admission_active_workers, committed, pressure) {
        Admission::Admit(reservation) => reservation,
        Admission::Defer(reason) => return Ok(LiveAdmission::Defer(reason)),
    };

    let lease_path = dir.join(format!("{}.lease", uuid::Uuid::new_v4()));
    let mut lease_file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&lease_path)?;
    lease_file.lock_exclusive()?;
    lease_file.write_all(encode_reservation(reservation).as_bytes())?;
    lease_file.sync_data()?;

    Ok(LiveAdmission::Admit(NodeCapacityLease {
        path: lease_path,
        _file: lease_file,
    }))
}

impl Drop for NodeCapacityLease {
    fn drop(&mut self) {
        // Serialize release with scans so a scanner never observes a directory
        // entry disappear midway through validation. On an I/O failure, leave
        // the path behind; once this File drops, its lock is released and the
        // next successful scan reclaims it as stale.
        let Some(dir) = self.path.parent() else {
            return;
        };
        let Ok(registry_lock) = open_registry_lock(dir) else {
            return;
        };
        if registry_lock.lock_exclusive().is_ok() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn open_registry_lock(dir: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join("registry.lock"))
}

fn open_existing_lease(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn capacity_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        Some(runtime_dir) => PathBuf::from(runtime_dir).join("gah-node-capacity"),
        None => std::env::temp_dir().join(format!(
            "gah-node-capacity-{}",
            // SAFETY: geteuid has no preconditions and cannot fail.
            unsafe { libc::geteuid() }
        )),
    }
}

fn encode_reservation(reservation: WorkerReservation) -> String {
    format!(
        "{} {:.3}\n",
        reservation.memory_bytes, reservation.cpu_units
    )
}

fn decode_reservation(encoded: &str) -> std::io::Result<WorkerReservation> {
    let mut fields = encoded.split_whitespace();
    let memory_bytes = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("invalid node-capacity memory reservation"))?;
    let cpu_units: f64 = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("invalid node-capacity CPU reservation"))?;
    if !cpu_units.is_finite() || cpu_units < 0.0 {
        return Err(std::io::Error::other(
            "node-capacity CPU reservation must be finite and non-negative",
        ));
    }
    if fields.next().is_some() {
        return Err(std::io::Error::other(
            "unexpected fields in node-capacity reservation",
        ));
    }
    Ok(WorkerReservation {
        memory_bytes,
        cpu_units,
    })
}

pub(crate) fn sample() -> std::io::Result<NodePressure> {
    // Integration tests execute the real `gah` binary, so `cfg(test)` is not
    // available inside that child. Debug builds accept an explicit fixture
    // file to make pressure-sensitive process tests deterministic. Release
    // binaries do not compile this branch and always read the live kernel.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("GAH_TEST_NODE_PRESSURE_FILE") {
        eprintln!(
            "gah loop: using explicit debug-only node pressure fixture {}",
            Path::new(&path).display()
        );
        let encoded = std::fs::read_to_string(path)?;
        return serde_json::from_str(&encoded).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid debug node pressure fixture: {error}"),
            )
        });
    }

    let meminfo = std::fs::read_to_string("/proc/meminfo")?;
    let memory_total_bytes = meminfo_kib(&meminfo, "MemTotal")
        .ok_or_else(|| std::io::Error::other("MemTotal missing from /proc/meminfo"))?
        .saturating_mul(1024);
    let memory_available_bytes = meminfo_kib(&meminfo, "MemAvailable")
        .ok_or_else(|| std::io::Error::other("MemAvailable missing from /proc/meminfo"))?
        .saturating_mul(1024);
    let load_one = std::fs::read_to_string("/proc/loadavg")?
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("one-minute load missing from /proc/loadavg"))?;

    Ok(NodePressure {
        memory_total_bytes,
        memory_available_bytes,
        logical_cpus: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        load_one,
        memory_full_psi_avg10: read_psi_avg10("/proc/pressure/memory", "full"),
        cpu_some_psi_avg10: read_psi_avg10("/proc/pressure/cpu", "some"),
    })
}

fn meminfo_kib(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key)
            .then(|| value.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}

fn read_psi_avg10(path: &str, class: &str) -> Option<f64> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != class {
            return None;
        }
        fields.find_map(|field| {
            field
                .strip_prefix("avg10=")
                .and_then(|value| value.parse().ok())
        })
    })
}

#[cfg(test)]
pub(crate) fn test_lease(dir: &Path) -> std::io::Result<(NodeCapacityLease, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("test-{}.lease", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)?;
    file.lock_exclusive()?;
    file.write_all(
        encode_reservation(WorkerReservation {
            memory_bytes: GIB,
            cpu_units: 0.5,
        })
        .as_bytes(),
    )?;
    Ok((
        NodeCapacityLease {
            path: path.clone(),
            _file: file,
        },
        path,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressure(available_gib: u64, cpus: usize, load_one: f64) -> NodePressure {
        NodePressure {
            memory_total_bytes: 16 * GIB,
            memory_available_bytes: available_gib * GIB,
            logical_cpus: cpus,
            load_one,
            memory_full_psi_avg10: Some(0.0),
            cpu_some_psi_avg10: Some(0.0),
        }
    }

    fn implementation() -> NextAction {
        NextAction::DispatchTicket {
            ticket_path: "ticket.md".into(),
            work_id: Some("#1".into()),
            recommended_backend: None,
            recommended_model: None,
            reason: "queued".into(),
        }
    }

    fn review() -> NextAction {
        NextAction::ReviewMr {
            work_id: Some("#2".into()),
            branch: "gah/review".into(),
            mr_url: None,
            reason: "queued".into(),
        }
    }

    #[test]
    fn idle_node_admits_multiple_workers_up_to_projected_memory() {
        let node = pressure(15, 10, 0.5);
        let mut committed = WorkerReservation::default();

        let Admission::Admit(first) = admission_for(&implementation(), 0, committed, node) else {
            panic!("first worker should be admitted");
        };
        committed += first;
        let Admission::Admit(second) = admission_for(&implementation(), 1, committed, node) else {
            panic!("second worker should be admitted");
        };
        committed += second;

        assert!(matches!(
            admission_for(&implementation(), 2, committed, node),
            Admission::Admit(_)
        ));
    }

    #[test]
    fn projected_launch_burst_preserves_memory_floor() {
        let node = pressure(15, 10, 0.5);
        let committed = WorkerReservation {
            memory_bytes: 12 * GIB,
            cpu_units: 6.0,
        };

        assert!(matches!(
            admission_for(&implementation(), 3, committed, node),
            Admission::Defer(reason) if reason.contains("memory reserve")
        ));
    }

    #[test]
    fn light_review_can_use_headroom_that_cannot_fit_another_implementation() {
        let node = pressure(5, 10, 1.0);
        let committed = WorkerReservation {
            memory_bytes: 4 * GIB,
            cpu_units: 2.0,
        };

        assert!(matches!(
            admission_for(&implementation(), 1, committed, node),
            Admission::Defer(_)
        ));
        assert!(matches!(
            admission_for(&review(), 1, committed, node),
            Admission::Admit(_)
        ));
    }

    #[test]
    fn cpu_pressure_stops_refill_but_not_first_safe_worker() {
        let node = pressure(12, 4, 3.2);
        assert!(matches!(
            admission_for(&implementation(), 0, WorkerReservation::default(), node),
            Admission::Admit(_)
        ));
        assert!(matches!(
            admission_for(
                &review(),
                1,
                WorkerReservation {
                    memory_bytes: GIB,
                    cpu_units: 0.5,
                },
                node
            ),
            Admission::Defer(reason) if reason.contains("CPU reserve")
        ));
    }

    #[test]
    fn critical_memory_and_psi_fail_closed() {
        let mut node = pressure(1, 10, 0.0);
        node.memory_available_bytes = 400 * 1024 * 1024;
        assert!(matches!(
            admission_for(&implementation(), 0, WorkerReservation::default(), node),
            Admission::Defer(reason) if reason.contains("critical")
        ));

        let mut node = pressure(12, 10, 0.0);
        node.memory_full_psi_avg10 = Some(1.5);
        assert!(matches!(
            admission_for(&review(), 0, WorkerReservation::default(), node),
            Admission::Defer(reason) if reason.contains("memory PSI")
        ));
    }

    #[test]
    fn parsers_accept_kernel_formats() {
        assert_eq!(
            meminfo_kib(
                "MemTotal:       16384000 kB\nMemAvailable: 100 kB\n",
                "MemTotal"
            ),
            Some(16_384_000)
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pressure");
        std::fs::write(
            &path,
            "some avg10=3.25 avg60=2.00 avg300=1.00 total=1\nfull avg10=0.75 avg60=0.50 avg300=0.10 total=2\n",
        )
        .unwrap();
        assert_eq!(read_psi_avg10(path.to_str().unwrap(), "full"), Some(0.75));
        assert_eq!(
            decode_reservation(&encode_reservation(WorkerReservation {
                memory_bytes: 4 * GIB,
                cpu_units: 2.0,
            }))
            .unwrap(),
            WorkerReservation {
                memory_bytes: 4 * GIB,
                cpu_units: 2.0,
            }
        );
    }

    #[test]
    fn reservation_decoder_rejects_non_finite_and_negative_cpu() {
        for encoded in [
            "1073741824 NaN\n",
            "1073741824 inf\n",
            "1073741824 -inf\n",
            "1073741824 -0.5\n",
        ] {
            let error =
                decode_reservation(encoded).expect_err("unsafe CPU reservation must fail closed");
            assert!(
                error.to_string().contains("finite and non-negative"),
                "unexpected error for {encoded:?}: {error}"
            );
        }
    }

    #[test]
    fn leases_coordinate_capacity_across_independent_workers_and_recover_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let node = pressure(15, 10, 0.5);
        let LiveAdmission::Admit(first) =
            acquire_in_dir(dir.path(), &implementation(), node, 0).unwrap()
        else {
            panic!("first worker should be admitted");
        };
        let LiveAdmission::Admit(_second) =
            acquire_in_dir(dir.path(), &implementation(), node, 0).unwrap()
        else {
            panic!("second worker should be admitted");
        };
        let LiveAdmission::Admit(_third) =
            acquire_in_dir(dir.path(), &implementation(), node, 0).unwrap()
        else {
            panic!("third worker should be admitted");
        };
        assert!(matches!(
            acquire_in_dir(dir.path(), &implementation(), node, 0).unwrap(),
            LiveAdmission::Defer(reason) if reason.contains("memory reserve")
        ));

        drop(first);
        assert!(matches!(
            acquire_in_dir(dir.path(), &review(), node, 0).unwrap(),
            LiveAdmission::Admit(_)
        ));
    }

    #[test]
    fn corrupt_live_lease_fails_admission_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.lease");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.lock_exclusive().unwrap();
        file.write_all(b"not-a-reservation\n").unwrap();
        file.sync_data().unwrap();

        let error = acquire_in_dir(dir.path(), &review(), pressure(15, 10, 0.5), 0)
            .expect_err("a corrupt live lease must not be ignored");
        assert!(error.to_string().contains("memory reservation"));
    }

    #[test]
    fn lease_drop_serializes_with_registry_scan_lock() {
        let dir = tempfile::tempdir().unwrap();
        let LiveAdmission::Admit(lease) =
            acquire_in_dir(dir.path(), &review(), pressure(15, 10, 0.5), 0).unwrap()
        else {
            panic!("review should be admitted");
        };
        let lease_path = lease.path.clone();
        let registry_lock = open_registry_lock(dir.path()).unwrap();
        registry_lock.lock_exclusive().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let dropper = std::thread::spawn(move || {
            drop(lease);
            done_tx.send(()).unwrap();
        });
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "lease release must wait for the registry scan lock"
        );
        FileExt::unlock(&registry_lock).unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        dropper.join().unwrap();
        assert!(!lease_path.exists());
    }

    #[test]
    fn vanished_lease_is_a_tolerated_release_race() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_existing_lease(&dir.path().join("already-gone.lease"))
            .unwrap()
            .is_none());
    }
}
