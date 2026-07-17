//! Transport for the `codex app-server` JSON-RPC wire format: a managed
//! child process over stdio, or a client connection to an existing
//! app-server control socket. Both send/receive newline-delimited JSON
//! and share the same reader/parser plumbing.
//!
//! Neither transport ever binds a listener: [`StdioTransport`] only
//! spawns a child and talks to its pipes, and [`UnixSocketTransport`]
//! only connects out to a path GAH did not create. This satisfies the
//! issue's "no non-loopback unauthenticated listener is created"
//! constraint by construction -- there is no listening socket at all.

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::runner::process::{kill_process_group, prepare_process_group};

use super::protocol::{IncomingMessage, ParseError};

/// One event surfaced from the background reader thread.
pub(crate) enum TransportEvent {
    Message(IncomingMessage),
    /// A line that failed to parse -- preserved rather than silently
    /// dropped so callers can decide whether it's fatal.
    Malformed {
        raw: String,
        reason: String,
    },
    /// The peer closed its end (process exited / socket closed).
    Closed(Option<i32>),
}

/// Behavior shared by the stdio and Unix-socket transports.
pub(crate) trait Transport: Send {
    fn send_line(&mut self, value: &Value) -> Result<()>;
    fn recv_timeout(&self, timeout: Duration) -> TransportEvent;
    fn shutdown(self: Box<Self>);
}

fn spawn_reader_thread<R: Read + Send + 'static>(
    reader: R,
    tx: Sender<TransportEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(TransportEvent::Closed(None));
                    break;
                }
                Err(_) => {
                    let _ = tx.send(TransportEvent::Closed(None));
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    if trimmed.is_empty() {
                        continue;
                    }
                    let event = match IncomingMessage::parse(trimmed) {
                        Ok(msg) => TransportEvent::Message(msg),
                        Err(ParseError::InvalidJson(reason))
                        | Err(ParseError::UnrecognizedShape(reason)) => TransportEvent::Malformed {
                            raw: trimmed.to_string(),
                            reason,
                        },
                    };
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn spawn_stderr_drain<R: Read + Send + 'static>(reader: R, log_path: PathBuf) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(file) = file.as_mut() {
                        let _ = file.write_all(crate::redact::redact(&line).as_bytes());
                    }
                }
            }
        }
    })
}

fn recv_with_timeout(rx: &Receiver<TransportEvent>, timeout: Duration) -> TransportEvent {
    match rx.recv_timeout(timeout) {
        Ok(event) => event,
        Err(RecvTimeoutError::Timeout) => TransportEvent::Closed(None),
        Err(RecvTimeoutError::Disconnected) => TransportEvent::Closed(None),
    }
}

/// A managed `codex app-server` child process communicating over its
/// stdin/stdout pipes. Runs in its own process group so shutdown/idle
/// handling matches the other backend adapters (see `runner::process`).
pub(crate) struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    events_rx: Receiver<TransportEvent>,
    reader_handle: Option<JoinHandle<()>>,
    stderr_handle: Option<JoinHandle<()>>,
}

impl StdioTransport {
    pub(crate) fn spawn(
        executable: &Path,
        args: &[String],
        worktree: &Path,
        env_vars: &[(String, String)],
        stderr_log_path: &Path,
    ) -> Result<Self> {
        let mut cmd = Command::new(executable);
        cmd.args(args)
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
        prepare_process_group(&mut cmd);

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "launching {}; is it installed and on PATH?",
                executable.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .context("codex app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("codex app-server stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("codex app-server stderr unavailable")?;

        let stderr_handle = spawn_stderr_drain(stderr, stderr_log_path.to_path_buf());

        let (tx, rx) = mpsc::channel();
        let reader_handle = spawn_reader_thread(stdout, tx);

        Ok(Self {
            child,
            stdin,
            events_rx: rx,
            reader_handle: Some(reader_handle),
            stderr_handle: Some(stderr_handle),
        })
    }
}

impl Transport for StdioTransport {
    fn send_line(&mut self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_string(value).context("serializing app-server message")?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .context("writing to codex app-server stdin")?;
        self.stdin
            .flush()
            .context("flushing codex app-server stdin")
    }

    fn recv_timeout(&self, timeout: Duration) -> TransportEvent {
        recv_with_timeout(&self.events_rx, timeout)
    }

    fn shutdown(mut self: Box<Self>) {
        let _ = kill_process_group(&mut self.child);
        let _ = self.child.wait();
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_handle.take() {
            let _ = handle.join();
        }
    }
}

/// A client connection to an already-running app-server control socket
/// (`codex app-server daemon start` / `proxy --sock`). GAH never listens
/// on this path -- it only connects out, so there is no unauthenticated
/// listener for this transport either.
#[cfg(unix)]
pub(crate) struct UnixSocketTransport {
    stream: UnixStream,
    events_rx: Receiver<TransportEvent>,
    reader_handle: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl UnixSocketTransport {
    pub(crate) fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .with_context(|| format!("connecting to {}", socket_path.display()))?;
        let read_half = stream
            .try_clone()
            .context("cloning app-server socket for reader thread")?;

        let (tx, rx) = mpsc::channel();
        let reader_handle = spawn_reader_thread(read_half, tx);

        Ok(Self {
            stream,
            events_rx: rx,
            reader_handle: Some(reader_handle),
        })
    }
}

#[cfg(unix)]
impl Transport for UnixSocketTransport {
    fn send_line(&mut self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_string(value).context("serializing app-server message")?;
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .context("writing to codex app-server socket")?;
        self.stream
            .flush()
            .context("flushing codex app-server socket")
    }

    fn recv_timeout(&self, timeout: Duration) -> TransportEvent {
        recv_with_timeout(&self.events_rx, timeout)
    }

    fn shutdown(mut self: Box<Self>) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    #[test]
    fn stdio_transport_round_trips_a_line() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "fake-app-server",
            "#!/bin/sh\nwhile IFS= read -r line; do\n  echo \"$line\"\ndone\n",
        );

        let mut transport = StdioTransport::spawn(
            &f.bin_dir.join("fake-app-server"),
            &[],
            &f.worktree,
            &[],
            &f.session_dir.join("stderr.log"),
        )
        .unwrap();

        transport
            .send_line(&serde_json::json!({"id": 1, "method": "ping", "params": {}}))
            .unwrap();

        let event = transport.recv_timeout(Duration::from_secs(5));
        match event {
            TransportEvent::Message(IncomingMessage::ServerRequest { id, method, .. }) => {
                assert_eq!(id, super::super::protocol::RequestId::Number(1));
                assert_eq!(method, "ping");
            }
            _ => panic!("expected the fake binary to echo the request back"),
        }

        Box::new(transport).shutdown();
    }

    #[test]
    fn stdio_transport_reports_closed_when_process_exits() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "fake-app-server", "#!/bin/sh\nexit 0\n");

        let transport = StdioTransport::spawn(
            &f.bin_dir.join("fake-app-server"),
            &[],
            &f.worktree,
            &[],
            &f.session_dir.join("stderr.log"),
        )
        .unwrap();

        let event = transport.recv_timeout(Duration::from_secs(5));
        assert!(matches!(event, TransportEvent::Closed(_)));

        Box::new(transport).shutdown();
    }

    #[test]
    fn stdio_transport_retains_malformed_lines_instead_of_dropping_them() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "fake-app-server",
            "#!/bin/sh\necho 'not json'\n",
        );

        let transport = StdioTransport::spawn(
            &f.bin_dir.join("fake-app-server"),
            &[],
            &f.worktree,
            &[],
            &f.session_dir.join("stderr.log"),
        )
        .unwrap();

        let event = transport.recv_timeout(Duration::from_secs(5));
        match event {
            TransportEvent::Malformed { raw, .. } => assert_eq!(raw, "not json"),
            _ => panic!("expected a malformed-line event"),
        }

        Box::new(transport).shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_transport_round_trips_a_line() {
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::TempDir::new().unwrap();
        let sock_path = tmp.path().join("fake.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            stream.write_all(line.as_bytes()).unwrap();
        });

        let mut transport = UnixSocketTransport::connect(&sock_path).unwrap();
        transport
            .send_line(&serde_json::json!({"id": 7, "method": "ping", "params": {}}))
            .unwrap();

        let event = transport.recv_timeout(Duration::from_secs(5));
        match event {
            TransportEvent::Message(IncomingMessage::ServerRequest { id, method, .. }) => {
                assert_eq!(id, super::super::protocol::RequestId::Number(7));
                assert_eq!(method, "ping");
            }
            _ => panic!("expected the fake socket server to echo the request back"),
        }

        Box::new(transport).shutdown();
        server.join().unwrap();
    }
}
