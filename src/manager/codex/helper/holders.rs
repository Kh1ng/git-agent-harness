use std::os::fd::RawFd;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub(crate) struct PipeIdentity {
        dev: u64,
        ino: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct ProcessIdentity {
        pid: u32,
        start_ticks: u64,
    }

    impl PipeIdentity {
        pub(crate) fn from_fd(fd: RawFd) -> Result<Self, String> {
            fs::metadata(format!("/proc/self/fd/{fd}"))
                .map(|metadata| Self {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                })
                .map_err(|error| format!("identifying helper pipe fd {fd}: {error}"))
        }
    }

    fn process_identity(pid: u32) -> Result<Option<ProcessIdentity>, String> {
        let path = format!("/proc/{pid}/stat");
        let stat = match fs::read_to_string(&path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("reading {path}: {error}")),
        };
        let fields = stat[stat
            .rfind(") ")
            .ok_or_else(|| format!("parsing {path}: missing command terminator"))?
            + 2..]
            .split_whitespace()
            .collect::<Vec<_>>();
        Ok(Some(ProcessIdentity {
            pid,
            start_ticks: fields
                .get(19)
                .ok_or_else(|| format!("parsing {path}: missing start time"))?
                .parse()
                .map_err(|error| format!("parsing {path} start time: {error}"))?,
        }))
    }

    fn deadline(deadline: Instant) -> Result<(), String> {
        (Instant::now() < deadline)
            .then_some(())
            .ok_or_else(|| "helper pipe-holder cleanup deadline elapsed".into())
    }

    fn holders(
        pipes: &HashSet<PipeIdentity>,
        deadline_at: Instant,
    ) -> Result<Vec<ProcessIdentity>, String> {
        let own_pid = std::process::id();
        let own_uid = unsafe { libc::geteuid() };
        let processes = fs::read_dir("/proc").map_err(|error| format!("reading /proc: {error}"))?;
        let mut holders = Vec::new();
        for entry in processes {
            deadline(deadline_at)?;
            let entry = entry.map_err(|error| format!("enumerating /proc: {error}"))?;
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == own_pid {
                continue;
            }
            let proc_path = entry.path();
            let metadata = match fs::metadata(&proc_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("inspecting {}: {error}", proc_path.display())),
            };
            if metadata.uid() != own_uid {
                continue;
            }
            let Some(identity) = process_identity(pid)? else {
                continue;
            };
            let fd_path = proc_path.join("fd");
            let fds = match fs::read_dir(&fd_path) {
                Ok(fds) => fds,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("inspecting {}: {error}", fd_path.display())),
            };
            let mut holds_pipe = false;
            for fd in fds {
                deadline(deadline_at)?;
                let fd = match fd {
                    Ok(fd) => fd,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(format!("enumerating {}: {error}", fd_path.display()))
                    }
                };
                match fs::metadata(fd.path()) {
                    Ok(metadata)
                        if pipes.contains(&PipeIdentity {
                            dev: metadata.dev(),
                            ino: metadata.ino(),
                        }) =>
                    {
                        holds_pipe = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("inspecting {}: {error}", fd.path().display()));
                    }
                }
            }
            if holds_pipe && process_identity(pid)? == Some(identity) {
                holders.push(identity);
            }
        }
        Ok(holders)
    }

    fn signal(identity: ProcessIdentity, signal: libc::c_int) -> Result<(), String> {
        if process_identity(identity.pid)? != Some(identity) {
            return Ok(());
        }
        if unsafe { libc::kill(identity.pid as libc::pid_t, signal) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!(
                    "signaling helper pipe holder {identity:?}: {error}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn terminate_pipe_holders(
        pipes: &[PipeIdentity],
        deadline_at: Instant,
    ) -> Result<(), String> {
        let pipes = pipes.iter().copied().collect::<HashSet<_>>();
        loop {
            deadline(deadline_at)?;
            let holders = holders(&pipes, deadline_at)?;
            if holders.is_empty() {
                return Ok(());
            }
            for holder in &holders {
                signal(*holder, libc::SIGSTOP)?;
            }
            for holder in holders {
                signal(holder, libc::SIGKILL)?;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::collections::HashSet;
    use std::mem::{size_of, MaybeUninit};

    const PROC_PIDFDPIPEINFO: libc::c_int = 6;
    const PROC_UID_ONLY: u32 = 4;
    const MAX_FDS_PER_PROCESS: usize = 4096;
    const MAX_PROCESSES: usize = 32 * 1024;

    #[repr(C)]
    struct PipeInfo {
        pipe_stat: libc::vinfo_stat,
        pipe_handle: u64,
        pipe_peerhandle: u64,
        pipe_status: i32,
        rfu_1: i32,
    }

    #[repr(C)]
    struct PipeFdInfo {
        pfi: ProcFileInfo,
        pipeinfo: PipeInfo,
    }

    #[repr(C)]
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: libc::off_t,
        fi_type: i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct ProcFdInfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub(crate) struct PipeIdentity(u64, u64);

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct ProcessIdentity {
        pid: i32,
        start_sec: u64,
        start_usec: u64,
    }

    impl PipeIdentity {
        pub(crate) fn from_fd(fd: RawFd) -> Result<Self, String> {
            let info = pipe_info(std::process::id() as i32, fd)?
                .ok_or_else(|| format!("helper pipe fd {fd} disappeared"))?;
            Ok(Self::new(
                info.pipeinfo.pipe_handle,
                info.pipeinfo.pipe_peerhandle,
            ))
        }

        fn new(first: u64, second: u64) -> Self {
            Self(first.min(second), first.max(second))
        }
    }

    const _: () = assert!(size_of::<ProcFileInfo>() == 24);
    const _: () = assert!(size_of::<PipeInfo>() == 160);
    const _: () = assert!(size_of::<PipeFdInfo>() == 184);

    fn process_info(pid: i32) -> Result<Option<(ProcessIdentity, u32, usize)>, String> {
        let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        if read == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return Err(format!("inspecting process {pid}: {error}"));
        }
        if read as usize != size_of::<libc::proc_bsdinfo>() {
            return Err(format!(
                "inspecting process {pid}: short BSD info ({read} bytes)"
            ));
        }
        let info = unsafe { info.assume_init() };
        Ok(Some((
            ProcessIdentity {
                pid,
                start_sec: info.pbi_start_tvsec,
                start_usec: info.pbi_start_tvusec,
            },
            info.pbi_uid,
            info.pbi_nfiles as usize,
        )))
    }

    fn pipe_info(pid: i32, fd: i32) -> Result<Option<PipeFdInfo>, String> {
        let mut info = MaybeUninit::<PipeFdInfo>::zeroed();
        let read = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDPIPEINFO,
                info.as_mut_ptr().cast(),
                size_of::<PipeFdInfo>() as i32,
            )
        };
        if read == 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::EBADF)) {
                return Ok(None);
            }
            return Err(format!("inspecting pipe fd {pid}/{fd}: {error}"));
        }
        if read as usize != size_of::<PipeFdInfo>() {
            return Err(format!(
                "inspecting pipe fd {pid}/{fd}: short info ({read} bytes)"
            ));
        }
        Ok(Some(unsafe { info.assume_init() }))
    }

    fn all_pids() -> Result<Vec<i32>, String> {
        let uid = unsafe { libc::geteuid() };
        let bytes = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
        if bytes <= 0 {
            return Err(format!(
                "listing processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        if bytes as usize > MAX_PROCESSES * size_of::<i32>() {
            return Err(format!("process list exceeds {MAX_PROCESSES} entries"));
        }
        let mut pids = vec![0_i32; bytes as usize / size_of::<i32>() + 32];
        let read = unsafe {
            libc::proc_listpids(
                PROC_UID_ONLY,
                uid,
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<i32>()) as i32,
            )
        };
        if read <= 0 {
            return Err(format!(
                "listing processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        let count = read as usize / size_of::<i32>();
        if count == pids.len() {
            return Err("listing processes exceeded bounded buffer".into());
        }
        pids.truncate(count);
        Ok(pids)
    }

    fn holders(
        pipes: &HashSet<PipeIdentity>,
        deadline: Instant,
    ) -> Result<Vec<ProcessIdentity>, String> {
        let own_pid = std::process::id() as i32;
        let own_uid = unsafe { libc::geteuid() };
        let mut holders = Vec::new();
        for pid in all_pids()? {
            if Instant::now() >= deadline {
                return Err("helper pipe-holder cleanup deadline elapsed".into());
            }
            if pid <= 0 || pid == own_pid {
                continue;
            }
            let Some((identity, uid, fd_count)) = process_info(pid)? else {
                continue;
            };
            if uid != own_uid {
                continue;
            }
            if fd_count > MAX_FDS_PER_PROCESS {
                return Err(format!(
                    "process {pid} has too many fds to inspect: {fd_count}"
                ));
            }
            let capacity = fd_count.saturating_add(16).max(16);
            let mut fds = vec![ProcFdInfo::default(); capacity];
            let read = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    (fds.len() * size_of::<ProcFdInfo>()) as i32,
                )
            };
            if read == 0 {
                if process_info(pid)?.is_none() {
                    continue;
                }
                return Err(format!(
                    "listing fds for process {pid}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let count = read as usize / size_of::<ProcFdInfo>();
            if read as usize % size_of::<ProcFdInfo>() != 0 {
                return Err(format!("fd list for process {pid} was misaligned"));
            }
            if count == fds.len() {
                return Err(format!("fd list for process {pid} exceeded bounded buffer"));
            }
            let mut holds_pipe = false;
            for fd in &fds[..count] {
                if Instant::now() >= deadline {
                    return Err("helper pipe-holder cleanup deadline elapsed".into());
                }
                if fd.proc_fdtype != libc::PROX_FDTYPE_PIPE as u32 {
                    continue;
                }
                let Some(info) = pipe_info(pid, fd.proc_fd)? else {
                    continue;
                };
                let pipe =
                    PipeIdentity::new(info.pipeinfo.pipe_handle, info.pipeinfo.pipe_peerhandle);
                if pipes.contains(&pipe) {
                    holds_pipe = true;
                    break;
                }
            }
            if holds_pipe && process_info(pid)?.is_some_and(|(current, _, _)| current == identity) {
                holders.push(identity);
            }
        }
        Ok(holders)
    }

    fn signal(identity: ProcessIdentity, signal: libc::c_int) -> Result<(), String> {
        if !process_info(identity.pid)?.is_some_and(|(current, _, _)| current == identity) {
            return Ok(());
        }
        if unsafe { libc::kill(identity.pid, signal) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!(
                    "signaling helper pipe holder {identity:?}: {error}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn terminate_pipe_holders(
        pipes: &[PipeIdentity],
        deadline: Instant,
    ) -> Result<(), String> {
        let pipes = pipes.iter().copied().collect::<HashSet<_>>();
        loop {
            if Instant::now() >= deadline {
                return Err("helper pipe-holder cleanup deadline elapsed".into());
            }
            let holders = holders(&pipes, deadline)?;
            if holders.is_empty() {
                return Ok(());
            }
            for holder in &holders {
                signal(*holder, libc::SIGSTOP)?;
            }
            for holder in holders {
                signal(holder, libc::SIGKILL)?;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(super) use platform::{terminate_pipe_holders, PipeIdentity};
