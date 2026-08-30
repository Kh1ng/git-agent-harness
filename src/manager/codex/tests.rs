use super::*;
use crate::runner::backends::test_util::{fixture, make_fake_bin};
use std::time::{Duration, Instant};

fn wait_for_updates(session: &mut CodexManagerSession, id: &GahSessionId) -> Vec<SessionUpdate> {
    for _ in 0..100 {
        let updates = session.stream(id).unwrap();
        if !updates.is_empty() {
            return updates;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for Codex updates");
}

fn wait_for_terminal(session: &mut CodexManagerSession, id: &GahSessionId) -> TerminalStatus {
    for _ in 0..100 {
        if let Some(status) = session.terminal_status(id).unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for Codex terminal status");
}

/// Unlike `wait_for_updates` (which returns as soon as a single
/// `stream()` call is non-empty), this waits out a short quiet window
/// after the last update seen -- a burst like "delta then usage" is
/// emitted via two separate notifications a moment apart, and under
/// full-suite parallel load those can land in two different `stream()`
/// polls. Tests that need *all* of a burst (not just proof that
/// something arrived) should use this instead.
fn drain_all_pending(session: &mut CodexManagerSession, id: &GahSessionId) -> Vec<SessionUpdate> {
    let mut all = Vec::new();
    let mut idle_polls = 0;
    for _ in 0..200 {
        let updates = session.stream(id).unwrap();
        if updates.is_empty() {
            if !all.is_empty() {
                idle_polls += 1;
                if idle_polls >= 3 {
                    break;
                }
            }
        } else {
            idle_polls = 0;
            all.extend(updates);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    all
}

/// Shell fragment answering `codex app-server generate-json-schema
/// --out DIR` the same way `detect_stable_methods` expects the real
/// binary to: writing a `ClientRequest.json` whose `oneOf[].method`
/// enums list the given method names. Kept as its own (non-`format!`)
/// string so its literal JSON braces don't need doubling for the outer
/// `format!` this gets spliced into.
fn schema_stub(methods: &[&str]) -> String {
    if methods.is_empty() {
        // Simulates schema generation itself failing (older binary
        // without the subcommand, or any other error) without falling
        // through into the RPC loop below and hanging on stdin.
        return r#"if [ "$1" = "app-server" ] && [ "$2" = "generate-json-schema" ]; then
  exit 1
fi
"#
        .to_string();
    }
    let entries: Vec<String> = methods
        .iter()
        .map(|m| format!(r#"{{"properties":{{"method":{{"enum":["{m}"]}}}}}}"#))
        .collect();
    format!(
        r#"if [ "$1" = "app-server" ] && [ "$2" = "generate-json-schema" ]; then
  shift 2
  outdir=""
  while [ $# -gt 0 ]; do
if [ "$1" = "--out" ]; then outdir="$2"; shift 2; else shift; fi
  done
  mkdir -p "$outdir"
  cat > "$outdir/ClientRequest.json" <<'JSON'
{{"oneOf":[{entries}]}}
JSON
  exit 0
fi
"#,
        entries = entries.join(",")
    )
}

/// A protocol-faithful fake `codex app-server`: real NDJSON framing,
/// real method names, real response/notification shapes -- verified
/// against the installed 0.145.0 binary's actual wire behavior (module
/// doc comment). `turn/start` acks immediately with the turn `inProgress`
/// and completes asynchronously via notifications on a background
/// thread, exactly like the real app-server. `stable_methods` controls
/// what `generate-json-schema` reports, which is what
/// `detect_stable_methods` uses to set `resume`/`interrupt` capabilities.
fn make_json_rpc_codex_with_methods(dir: &Path, record_dir: &Path, stable_methods: &[&str]) {
    let body = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'codex-cli 1.2.3'
  exit 0
fi
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  echo 'Logged in using ChatGPT'
  echo 'token=super-secret' >&2
  exit 0
fi
{schema_stub}
if [ "$1" != "app-server" ]; then
  exit 1
fi

tmp="$(mktemp)"
cat > "$tmp" <<'PY'
import json
import os
import sys
import threading
import time

record = os.environ["RECORD_PATH"]
lock = threading.Lock()

def emit(obj):
    with lock:
        print(json.dumps(obj), flush=True)

thread_counter = 0
turn_counter = 0
interrupted_turns = set()

def run_turn(thread_id, turn_id, text):
    emit({{
        "method": "item/agentMessage/delta",
        "params": {{"threadId": thread_id, "turnId": turn_id, "delta": "reply: " + text}},
    }})
    emit({{
        "method": "thread/tokenUsage/updated",
        "params": {{
            "threadId": thread_id,
            "turnId": turn_id,
            "tokenUsage": {{"total": {{"totalTokens": 12}}, "modelContextWindow": 4096}},
        }},
    }})
    if text == "fail":
        emit({{
            "method": "turn/completed",
            "params": {{
                "threadId": thread_id,
                "turn": {{"id": turn_id, "status": "failed", "error": {{"message": "turn failed"}}}},
            }},
        }})
        return
    if text == "delayed-old":
        time.sleep(0.2)
    if text == "slow":
        time.sleep(1)
    with lock:
        if turn_id in interrupted_turns:
            return
    emit({{
        "method": "turn/completed",
        "params": {{"threadId": thread_id, "turn": {{"id": turn_id, "status": "completed", "error": None}}}},
    }})

with open(record, "a", encoding="utf-8") as fh:
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        fh.write(line + "\n")
        fh.flush()
        msg = json.loads(line)
        method = msg.get("method")
        req_id = msg.get("id")

        if method == "initialize":
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{}}}})
        elif method == "initialized":
            pass
        elif method == "thread/start":
            thread_counter += 1
            thread_id = "thread-" + str(thread_counter)
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"thread": {{"id": thread_id}}}}}})
        elif method == "thread/resume":
            thread_id = msg["params"]["threadId"]
            if thread_id == "thread-fail":
                emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32000, "message": "resume failed"}}}})
            else:
                emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"thread": {{"id": thread_id}}}}}})
        elif method == "turn/start":
            turn_counter += 1
            turn_id = "turn-" + str(turn_counter)
            thread_id = msg["params"]["threadId"]
            text = msg["params"]["input"][0]["text"]
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"turn": {{"id": turn_id, "status": "inProgress"}}}}}})
            threading.Thread(target=run_turn, args=(thread_id, turn_id, text), daemon=True).start()
        elif method == "turn/steer":
            thread_id = msg["params"]["threadId"]
            expected_turn_id = msg["params"]["expectedTurnId"]
            text = msg["params"]["input"][0]["text"]
            # Real Codex returns the *same* turnId from a successful steer
            # (verified live) and errors "no active turn to steer" once the
            # turn has ended -- the magic "steer-ok" text is this fake's
            # stand-in for "the turn is still genuinely active".
            if text == "steer-ok":
                emit({{"jsonrpc": "2.0", "id": req_id, "result": {{"turnId": expected_turn_id}}}})
                emit({{
                    "method": "item/agentMessage/delta",
                    "params": {{"threadId": thread_id, "turnId": expected_turn_id, "delta": "steered: " + text}},
                }})
            else:
                emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32600, "message": "no active turn to steer"}}}})
        elif method == "turn/interrupt":
            thread_id = msg["params"]["threadId"]
            turn_id = msg["params"]["turnId"]
            with lock:
                interrupted_turns.add(turn_id)
            emit({{"jsonrpc": "2.0", "id": req_id, "result": {{}}}})
            emit({{
                "method": "turn/completed",
                "params": {{"threadId": thread_id, "turn": {{"id": turn_id, "status": "interrupted", "error": None}}}},
            }})
        else:
            emit({{"jsonrpc": "2.0", "id": req_id, "error": {{"code": -32601, "message": "unknown method"}}}})
PY
export RECORD_PATH='{record_path}'
exec python3 -u "$tmp" "$@"
"#,
        schema_stub = schema_stub(stable_methods),
        record_path = record_dir.join("requests.jsonl").display()
    );
    make_fake_bin(dir, "codex", &body);
}

/// Default fake: reports every stable method this adapter uses, so
/// existing tests keep exercising the "fully capable" path unless they
/// opt into a narrower method set via `make_json_rpc_codex_with_methods`.
fn make_json_rpc_codex(dir: &Path, record_dir: &Path) {
    make_json_rpc_codex_with_methods(
        dir,
        record_dir,
        &[
            "thread/start",
            "thread/resume",
            "turn/start",
            "turn/steer",
            "turn/interrupt",
        ],
    );
}

#[test]
fn discover_reports_version_and_redacted_auth_state() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_fake_bin(
        &f.bin_dir,
        "codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"login\" ] && [ \"$2\" = \"status\" ]; then echo 'Logged in using ChatGPT'; echo 'token=super-secret' >&2; exit 0; fi\nexit 1\n",
    );

    let discovery = discover(f.bin_dir.join("codex")).unwrap();
    assert_eq!(discovery.version.as_deref(), Some("codex-cli 1.2.3"));
    assert_eq!(discovery.auth_state, CodexAuthState::LoggedIn);
}

#[test]
fn discovery_does_not_retain_failed_status_output() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_fake_bin(
        &f.bin_dir,
        "codex",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\necho 'Authorization: Bearer sk-secret' >&2\nexit 1\n",
    );

    let discovery = discover(f.bin_dir.join("codex")).unwrap();
    let CodexAuthState::Error(message) = discovery.auth_state else {
        panic!("expected failed status discovery");
    };
    assert!(!message.contains("sk-secret"));
}

#[test]
fn session_directory_requires_absolute_user_state() {
    assert_eq!(
        resolve_session_dir(Some(OsStr::new("/state")), Some(OsStr::new("/home/user"))).unwrap(),
        PathBuf::from("/state/gah/manager-sessions/codex")
    );
    assert_eq!(
        resolve_session_dir(Some(OsStr::new("")), Some(OsStr::new("/home/user"))).unwrap(),
        PathBuf::from("/home/user/.local/state/gah/manager-sessions/codex")
    );
    assert!(resolve_session_dir(Some(OsStr::new("relative")), None).is_err());
}

#[test]
fn rpc_errors_surface_structured_detail() {
    let error = decode_json_rpc_response(json!({
        "error": {"code": -32000, "message": "thread not found"}
    }))
    .unwrap_err();
    assert_eq!(error.to_string(), "thread not found (code -32000)");
}

#[test]
fn adapter_runs_turn_and_streams_message_chunks() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);

    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();
    let updates = wait_for_updates(&mut session, &id);
    assert_eq!(
        updates,
        vec![
            SessionUpdate::MessageChunk("reply: hello".into()),
            SessionUpdate::Usage {
                used: 12,
                size: 4096
            }
        ]
    );
    assert_eq!(
        wait_for_terminal(&mut session, &id),
        TerminalStatus::Completed
    );
}

#[test]
fn turn_returns_before_completion_and_can_be_interrupted() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();

    let started = Instant::now();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    session.interrupt(&id).unwrap();
    assert_eq!(
        session.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Interrupted)
    );
}

#[test]
fn turn_failure_becomes_terminal_failure() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "fail".into(),
        })
        .unwrap();

    let TerminalStatus::Failed(message) = wait_for_terminal(&mut session, &id) else {
        panic!("expected failed turn status");
    };
    assert!(message.contains("turn failed"));
}

#[test]
fn failed_mapping_commit_starts_no_remote_turn() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("not-a-directory");
    fs::write(&session_dir, "occupied").unwrap();
    let mut session =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();

    assert!(session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "must-not-start".into(),
        })
        .is_err());
    assert!(session.sessions.is_empty());

    let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
    assert!(
        !requests.contains("turn/start"),
        "mapping persistence must succeed before any paid turn starts"
    );
    assert!(
        !requests.contains("turn/interrupt"),
        "there is no remote turn to clean up when persistence fails first"
    );
}

#[test]
fn failed_initial_turn_start_does_not_leave_an_orphaned_local_session() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    session.transport.fail_request = Some("turn/start");

    let result = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .is_err();
    assert!(result);
    assert!(session.sessions.is_empty());
    let entries = fs::read_dir(f.record_dir.join("sessions"))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        entries.is_empty(),
        "failed turn/start must roll back the already-durable mapping"
    );
}

#[test]
fn durable_mapping_is_created_with_owner_only_permissions() {
    use std::os::unix::fs::MetadataExt;

    let state = tempfile::TempDir::new().unwrap();
    let id = GahSessionId::new("profile-a");
    persist_mapping(state.path(), &id, "thread-1").unwrap();

    let mode = fs::metadata(mapping_path(state.path(), &id))
        .unwrap()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn adapter_resume_and_interrupt_round_trip() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);

    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();
    session.resume(&id).unwrap();
    session.interrupt(&id).unwrap();
    assert_eq!(
        session.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Interrupted)
    );
}

#[test]
fn send_steers_an_active_turn_instead_of_starting_a_new_one() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();
    drain_all_pending(&mut session, &id); // drain the initial turn's own reply/usage

    session.send(&id, "steer-ok").unwrap();
    let updates = drain_all_pending(&mut session, &id);
    assert_eq!(
        updates,
        vec![SessionUpdate::MessageChunk("steered: steer-ok".into())]
    );

    let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
    assert_eq!(
        requests.matches("\"turn/steer\"").count(),
        1,
        "send against an active turn must use turn/steer exactly once"
    );
    assert_eq!(
        requests.matches("\"turn/start\"").count(),
        1,
        "the steered message must not additionally start a fresh turn -- only the \
         initial start() call's turn/start should appear"
    );
}

#[test]
fn send_falls_back_to_turn_start_when_steer_races_against_turn_completion() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();
    drain_all_pending(&mut session, &id);

    // Any text other than the fake's "steer-ok" magic value makes the
    // fake respond to turn/steer the way the real app-server does once
    // a turn has already ended: "no active turn to steer". `send` must
    // recover from that by starting a fresh turn, not by propagating
    // the error.
    session.send(&id, "not-active-anymore").unwrap();
    let updates = drain_all_pending(&mut session, &id);
    assert_eq!(
        updates,
        vec![
            SessionUpdate::MessageChunk("reply: not-active-anymore".into()),
            SessionUpdate::Usage {
                used: 12,
                size: 4096
            }
        ]
    );

    let requests = fs::read_to_string(f.record_dir.join("requests.jsonl")).unwrap();
    assert!(
        requests.contains("turn/steer"),
        "the race-triggering turn/steer attempt must still have been sent"
    );
}

#[test]
fn delayed_old_completion_cannot_terminate_replacement_turn() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "delayed-old".into(),
        })
        .unwrap();
    drain_all_pending(&mut session, &id);

    // The fake rejects this steer as if turn A finished server-side,
    // so send() starts replacement turn B. A's completion notification
    // is deliberately delayed until after B is acknowledged.
    session.send(&id, "slow").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(session.inspect(&id).unwrap(), SessionStatus::Working);
    assert_eq!(
        session.sessions[&id].active_turn_id.as_deref(),
        Some("turn-2"),
        "turn A's delayed completion must not clear replacement turn B"
    );

    session.interrupt(&id).unwrap();
}

#[test]
fn capabilities_reflect_methods_the_installed_binary_actually_reports() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    // No thread/resume, no turn/interrupt in the fake's schema output --
    // an older/narrower app-server than the one this module was
    // verified against.
    make_json_rpc_codex_with_methods(
        &f.bin_dir,
        &f.record_dir,
        &["thread/start", "turn/start", "turn/steer"],
    );
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();

    assert_eq!(
        session.capabilities(),
        SessionCapabilities {
            resume: false,
            interrupt: false,
            inspect: true,
        }
    );

    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();
    assert!(crate::manager::unsupported_capability(&session.resume(&id).unwrap_err()).is_some());
    assert!(crate::manager::unsupported_capability(&session.interrupt(&id).unwrap_err()).is_some());
}

#[test]
fn capabilities_fail_closed_when_schema_detection_itself_fails() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    // `generate-json-schema` exits nonzero (e.g. a Codex old enough the
    // subcommand doesn't behave as expected) while the app-server RPC
    // loop itself still works fine -- construction must still succeed,
    // but with every schema-dependent capability honestly false rather
    // than guessed true.
    make_json_rpc_codex_with_methods(&f.bin_dir, &f.record_dir, &[]);
    let session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();

    assert_eq!(
        session.capabilities(),
        SessionCapabilities {
            resume: false,
            interrupt: false,
            inspect: true,
        }
    );
}

#[test]
fn adapter_passes_shared_contract_suite() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    crate::manager::contract::run_contract_suite(&mut session);
}

#[test]
fn restart_resumes_through_durable_gah_to_codex_mapping() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let id = {
        let mut adapter =
            CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir)
                .unwrap();
        adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap()
    };
    assert!(!id.as_str().contains("thread-"));

    let restored_id = id.as_str().parse::<GahSessionId>().unwrap();
    let mut restarted =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();
    restarted.resume(&restored_id).unwrap();
    restarted.send(&restored_id, "after restart").unwrap();
    assert!(wait_for_updates(&mut restarted, &restored_id)
        .contains(&SessionUpdate::MessageChunk("reply: after restart".into())));
}

#[test]
fn failed_restart_resume_does_not_unlock_send() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let id = GahSessionId::new("profile-a");
    persist_mapping(&session_dir, &id, "thread-fail").unwrap();
    let mut restarted =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();

    assert!(restarted.resume(&id).is_err());
    assert!(restarted
        .send(&id, "must not bypass resume")
        .unwrap_err()
        .to_string()
        .contains("must be resumed"));
}

/// Bounded handshake smoke test against whatever `codex` binary is
/// actually installed -- proves the wire protocol this module hardcodes
/// (method names, param shapes, response shapes) still matches a real
/// `codex app-server`, without running a real model turn (no
/// `turn/start`), so it doesn't spend API budget on every run. Gated
/// behind an env var, like `installed_hermes_passes_shared_contract_when_requested`
/// in `manager::hermes`, since most CI environments won't have `codex`
/// installed or authenticated.
#[test]
fn installed_codex_passes_handshake_smoke_when_requested() {
    let Some(executable) = std::env::var_os("GAH_TEST_REAL_CODEX") else {
        return;
    };
    let _exec_guard = crate::test_support::ExecGuard::new();
    let discovery = discover(&executable).unwrap();
    assert!(
        discovery.version.is_some(),
        "installed Codex must report a version"
    );

    let mut transport = CodexTransport::spawn(Path::new(&executable)).unwrap();
    rpc_request(
        &mut transport,
        "initialize",
        json!({
            "clientInfo": {
                "name": CLIENT_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
    .unwrap();
    transport
        .send_notification("initialized", json!({}))
        .unwrap();

    let response = rpc_request(
        &mut transport,
        "thread/start",
        json!({
            "cwd": std::env::temp_dir(),
            "approvalPolicy": "never",
        }),
    )
    .unwrap();
    let thread_id = response
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str);
    assert!(
        thread_id.is_some(),
        "real Codex app-server must return a thread id from thread/start"
    );

    let _ = crate::runner::process::kill_process_group(&mut transport.child);
    let _ = transport.child.wait();
}

/// Drains `stream()` until `terminal_status()` reports terminal,
/// accumulating every update seen along the way. Bounded by `timeout`
/// rather than the fake-server tests' fixed 100x10ms budget, since a
/// real model turn's latency is real network+inference time, not a
/// background thread we control.
fn drain_until_terminal(
    session: &mut CodexManagerSession,
    id: &GahSessionId,
    timeout: Duration,
) -> (Vec<SessionUpdate>, TerminalStatus) {
    let deadline = Instant::now() + timeout;
    let mut updates = Vec::new();
    loop {
        updates.extend(session.stream(id).unwrap());
        if let Some(status) = session.terminal_status(id).unwrap() {
            updates.extend(session.stream(id).unwrap());
            return (updates, status);
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for real Codex turn to reach a terminal status");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Bounded real-Codex lifecycle smoke test, gated the same way as
/// `installed_codex_passes_handshake_smoke_when_requested` above but
/// going through `CodexManagerSession` itself (not raw transport) for a
/// real `turn/start`: structured message-chunk and token-usage
/// streaming, a `Completed` terminal status, a full process restart
/// resuming purely from the durable on-disk thread mapping, a second
/// real turn after that resume, and a real `turn/interrupt` against a
/// turn proven active by already having streamed real content (the
/// module doc comment's live handshake found interrupting *before* any
/// content streams back can race Codex's own turn-activation and
/// return "no active turn to interrupt" -- waiting for the first delta
/// is the same margin that trace needed).
///
/// `turn/steer` is deliberately not exercised here: doing so
/// deterministically needs the same wait-for-first-token pattern
/// layered on top of a *second* concurrently in-flight turn, which
/// risks turning this bounded, budget-conscious smoke test into a flaky
/// and budget-unbounded one. It was instead verified directly against
/// the wire during development: a real `turn/steer` sent after the
/// turn had streamed content succeeded, returning the same `turnId`
/// (not a new one) with the turn continuing to stream under it, and a
/// `turn/steer` sent before any content streamed back reproducibly hit
/// the same "no active turn to steer" race `send`'s fallback handles.
#[test]
fn installed_codex_adapter_completes_a_real_turn_and_resumes_after_restart_when_requested() {
    let Some(executable) = std::env::var_os("GAH_TEST_REAL_CODEX") else {
        return;
    };
    let _exec_guard = crate::test_support::ExecGuard::new();
    let state = tempfile::TempDir::new().unwrap();
    let session_dir = state.path().join("manager-sessions");
    let turn_timeout = Duration::from_secs(30);

    let id = {
        let mut session =
            CodexManagerSession::new_with_session_dir(&executable, &session_dir).unwrap();
        let id = session
            .start(StartRequest {
                profile: "gah-test-real-codex".into(),
                instruction: "Reply with exactly: pong".into(),
            })
            .unwrap();
        let (updates, status) = drain_until_terminal(&mut session, &id, turn_timeout);
        assert_eq!(status, TerminalStatus::Completed);
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, SessionUpdate::MessageChunk(text) if text.contains("pong"))),
            "expected a real agent-message delta containing 'pong', got {updates:?}"
        );
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, SessionUpdate::Usage { .. })),
            "expected a real token-usage update, got {updates:?}"
        );
        id
    };

    // Restart: drop the adapter/process entirely, resume purely from
    // the durable GAH-session-id -> Codex-thread-id mapping on disk.
    let mut restarted =
        CodexManagerSession::new_with_session_dir(&executable, &session_dir).unwrap();
    restarted.resume(&id).unwrap();
    restarted.send(&id, "Reply with exactly: ping").unwrap();
    let (updates, status) = drain_until_terminal(&mut restarted, &id, turn_timeout);
    assert_eq!(status, TerminalStatus::Completed);
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::MessageChunk(text) if text.contains("ping"))),
        "expected a real agent-message delta containing 'ping' after restart+resume, got {updates:?}"
    );

    // Interrupt: only sent once real content has streamed back, so the
    // turn is unambiguously active server-side (see doc comment above).
    let interrupt_id = restarted
        .start(StartRequest {
            profile: "gah-test-real-codex".into(),
            instruction: "Count slowly from 1 to 500, one number per line, no other text.".into(),
        })
        .unwrap();
    let deadline = Instant::now() + turn_timeout;
    loop {
        let updates = restarted.stream(&interrupt_id).unwrap();
        if updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::MessageChunk(_)))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for real Codex to stream before interrupting"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    restarted.interrupt(&interrupt_id).unwrap();
    let (_, status) = drain_until_terminal(&mut restarted, &interrupt_id, turn_timeout);
    assert_eq!(status, TerminalStatus::Interrupted);
}
