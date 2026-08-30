use super::*;
use crate::manager::{
    unsupported_capability, GahSessionId, ManagerSession, SessionUpdate, StartRequest,
    TerminalStatus,
};
use crate::runner::backends::test_util::{fixture, make_fake_bin};
use std::fs;
use std::time::{Duration, Instant};

#[test]
fn discovery_records_only_whitelisted_version_and_auth_fields() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_fake_bin(
        &f.bin_dir,
        "claude",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '2.1.197 (Claude Code)'
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "status" ] && [ "$3" = "--json" ]; then
  echo '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"pro","accessToken":"super-secret"}'
  echo 'Authorization: Bearer super-secret' >&2
  exit 0
fi
exit 1
"#,
    );

    let discovery = discover(f.bin_dir.join("claude")).unwrap();
    assert_eq!(discovery.executable, f.bin_dir.join("claude"));
    assert_eq!(discovery.version.as_deref(), Some("2.1.197 (Claude Code)"));
    assert_eq!(
        discovery.auth_state,
        ClaudeAuthState::LoggedIn {
            auth_method: Some("claude.ai".into()),
            api_provider: Some("firstParty".into()),
            subscription_type: Some("pro".into()),
        }
    );
    assert!(!format!("{discovery:?}").contains("super-secret"));
}

#[test]
fn failed_auth_discovery_does_not_retain_command_output() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\necho 'token=super-secret' >&2\nexit 7\n",
        );

    let discovery = discover(f.bin_dir.join("claude")).unwrap();
    let ClaudeAuthState::Error(message) = discovery.auth_state else {
        panic!("expected a redacted discovery failure");
    };
    assert!(message.contains("status 7"));
    assert!(!message.contains("super-secret"));
}

fn make_streaming_claude(dir: &Path, record_dir: &Path) {
    let script = format!(
        r#"#!/usr/bin/env python3
import json
import os
import sys
import time

record_path = {record_path:?}
sentinel_path = {sentinel_path:?}
eof_alive_path = {eof_alive_path:?}
args = sys.argv[1:]
if args == ["--version"]:
    print("2.1.197 (Claude Code)")
    raise SystemExit(0)
if args == ["auth", "status", "--json"]:
    print(json.dumps({{
        "loggedIn": True,
        "authMethod": "claude.ai",
        "apiProvider": "firstParty",
        "subscriptionType": "pro",
        "accessToken": "must-not-be-retained",
    }}))
    raise SystemExit(0)

with open(record_path, "a", encoding="utf-8") as record:
    record.write(json.dumps(args) + "\n")
    record.flush()

def arg_value(flag):
    try:
        return args[args.index(flag) + 1]
    except (ValueError, IndexError):
        return None

session_id = arg_value("--session-id") or arg_value("--resume")
print(json.dumps({{
    "type": "system",
    "subtype": "init",
    "session_id": session_id,
    "claude_code_version": "2.1.197",
}}), flush=True)
print(json.dumps({{
    "type": "rate_limit_event",
    "rate_limit_info": {{"status": "allowed"}},
}}), flush=True)

for raw in sys.stdin:
    message = json.loads(raw)
    prompt = message["message"]["content"]
    if prompt == "malformed":
        print("not-valid-json-garbage", flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "must never be observed",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    if prompt == "mismatched-session":
        print(json.dumps({{
            "type": "assistant",
            "session_id": "00000000-0000-0000-0000-000000000000",
            "message": {{"content": [{{"type": "text", "text": "must never be observed"}}]}},
        }}), flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "must never complete",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    invalid_events = {{
        "missing-session": {{
            "type": "assistant",
            "message": {{"content": []}},
        }},
        "malformed-assistant": {{
            "type": "assistant",
            "session_id": session_id,
            "message": {{}},
        }},
        "malformed-result": {{
            "type": "result",
            "session_id": session_id,
            "subtype": "success",
        }},
        "malformed-stream": {{
            "type": "stream_event",
            "session_id": session_id,
            "event": {{"type": "content_block_delta"}},
        }},
        "malformed-tool-progress": {{
            "type": "tool_progress",
            "session_id": session_id,
            "tool_use_id": "toolu_test",
            "tool_name": "Read",
        }},
        "malformed-tool-summary": {{
            "type": "tool_use_summary",
            "session_id": session_id,
            "summary": "Read one file",
            "preceding_tool_use_ids": [42],
        }},
        "unknown-event": {{
            "type": "reslt",
            "session_id": session_id,
        }},
    }}
    if prompt in invalid_events:
        print(json.dumps(invalid_events[prompt]), flush=True)
        time.sleep(10)
        continue
    if prompt == "close-output":
        with open(eof_alive_path, "w", encoding="utf-8") as fh:
            fh.write("stdout closed while child remains alive")
        os.close(sys.stdout.fileno())
        time.sleep(10)
        continue
    if prompt == "slow":
        time.sleep(3)
        with open(sentinel_path, "w", encoding="utf-8") as fh:
            fh.write("completed")
        print(json.dumps({{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "slow turn completed",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}), flush=True)
        continue
    if prompt == "delayed-fail":
        time.sleep(0.2)
    if prompt == "fail" or prompt == "delayed-fail":
        print(json.dumps({{
            "type": "assistant",
            "session_id": session_id,
            "message": {{"content": [{{"type": "text", "text": "presentation failure prose"}}]}},
        }}), flush=True)
        print(json.dumps({{
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": True,
            "session_id": session_id,
            "errors": ["structured protocol failure"],
            "usage": {{"input_tokens": 10, "output_tokens": 2,
                       "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "model_error",
        }}), flush=True)
        continue
    if prompt == "tool-events":
        print(json.dumps({{
            "type": "tool_progress",
            "tool_use_id": "toolu_test",
            "tool_name": "Read",
            "parent_tool_use_id": None,
            "elapsed_time_seconds": 1.5,
            "uuid": "11111111-1111-4111-8111-111111111111",
            "session_id": session_id,
        }}), flush=True)
        print(json.dumps({{
            "type": "tool_use_summary",
            "summary": "Read one file",
            "preceding_tool_use_ids": ["toolu_test"],
            "uuid": "22222222-2222-4222-8222-222222222222",
            "session_id": session_id,
        }}), flush=True)
    if prompt == "duplicate-result":
        result = {{
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": session_id,
            "result": "duplicate result",
            "usage": {{"input_tokens": 1, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
            "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
            "terminal_reason": "completed",
        }}
        print(json.dumps(result), flush=True)
        print(json.dumps(result), flush=True)
        time.sleep(10)
        continue
    reply = "reply: " + prompt
    print(json.dumps({{
        "type": "stream_event",
        "session_id": session_id,
        "event": {{"type": "content_block_delta",
                  "delta": {{"type": "text_delta", "text": reply}}}},
    }}), flush=True)
    print(json.dumps({{
        "type": "assistant",
        "session_id": session_id,
        "message": {{"content": [{{"type": "text", "text": reply}}]}},
    }}), flush=True)
    print(json.dumps({{
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "session_id": session_id,
        "result": reply,
        "usage": {{"input_tokens": 10, "output_tokens": 2,
                   "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}},
        "modelUsage": {{"claude-test": {{"contextWindow": 200000}}}},
        "terminal_reason": "completed",
    }}), flush=True)
    if prompt == "complete-then-close":
        os.close(sys.stdout.fileno())
        time.sleep(10)
        continue
"#,
        record_path = record_dir.join("argv.jsonl").display().to_string(),
        sentinel_path = record_dir
            .join("slow-completed.marker")
            .display()
            .to_string(),
        eof_alive_path = record_dir
            .join("eof-child-alive.marker")
            .display()
            .to_string(),
    );
    make_fake_bin(dir, "claude", &script);
}

fn wait_for_turn(
    session: &mut ClaudeManagerSession,
    id: &GahSessionId,
) -> (Vec<SessionUpdate>, TerminalStatus) {
    let mut updates = Vec::new();
    for _ in 0..200 {
        updates.extend(session.stream(id).unwrap());
        if let Some(status) = session.terminal_status(id).unwrap() {
            updates.extend(session.stream(id).unwrap());
            return (updates, status);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for Claude's structured result event");
}

#[test]
fn structured_stream_drives_output_usage_and_completed_lifecycle() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let mut adapter =
        ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir).unwrap();

    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();
    let (updates, status) = wait_for_turn(&mut adapter, &id);

    assert_eq!(status, TerminalStatus::Completed);
    assert_eq!(
        updates,
        vec![
            SessionUpdate::MessageChunk("reply: hello".into()),
            SessionUpdate::Usage {
                used: 19,
                size: 200_000,
            },
        ]
    );
}

#[test]
fn structured_result_error_becomes_terminal_failure() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "fail".into(),
        })
        .unwrap();

    let (_, TerminalStatus::Failed(message)) = wait_for_turn(&mut adapter, &id) else {
        panic!("expected Claude's structured error result to fail the turn");
    };
    assert!(message.contains("error_during_execution"));
    assert!(message.contains("structured protocol failure"));
    assert!(!message.contains("presentation failure prose"));
}

#[test]
fn restart_resumes_with_the_durable_provider_session_id() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let id = {
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: "hello".into(),
            })
            .unwrap();
        let _ = wait_for_turn(&mut adapter, &id);
        id
    };

    let mut restarted =
        ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir).unwrap();
    restarted.resume(&id).unwrap();
    restarted.send(&id, "after restart").unwrap();
    let (updates, status) = wait_for_turn(&mut restarted, &id);
    assert_eq!(status, TerminalStatus::Completed);
    assert!(updates.contains(&SessionUpdate::MessageChunk("reply: after restart".into())));

    let invocations: Vec<Vec<String>> = fs::read_to_string(f.record_dir.join("argv.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let started_provider_id = invocations[0]
        .windows(2)
        .find(|pair| pair[0] == "--session-id")
        .unwrap()[1]
        .clone();
    let resumed_provider_id = invocations[1]
        .windows(2)
        .find(|pair| pair[0] == "--resume")
        .unwrap()[1]
        .clone();
    assert_eq!(resumed_provider_id, started_provider_id);
    assert_ne!(resumed_provider_id, id.as_str());
}

#[test]
fn adapter_passes_the_shared_contract_suite() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();

    crate::manager::contract::run_contract_suite(&mut adapter);
}

#[test]
fn unsupported_inspect_fails_with_the_typed_error() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();

    assert!(unsupported_capability(&adapter.inspect(&id).unwrap_err()).is_some());
}

fn wait_for_terminal_status(
    adapter: &mut ClaudeManagerSession,
    id: &GahSessionId,
) -> TerminalStatus {
    for _ in 0..300 {
        let _ = adapter.stream(id);
        if let Ok(Some(status)) = adapter.terminal_status(id) {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for a terminal status");
}

#[test]
fn capabilities_advertise_real_interrupt_support() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    assert!(adapter.capabilities().interrupt);
}

#[test]
fn eof_with_outstanding_work_fails_and_kills_a_live_child() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let alive = f.record_dir.join("eof-child-alive.marker");
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "close-output".into(),
        })
        .unwrap();
    for _ in 0..100 {
        if alive.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        alive.exists(),
        "fake child must still be alive after closing stdout"
    );

    let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
        panic!("expected EOF with outstanding work to fail the session");
    };
    assert!(message.contains("closed its structured output"));
    assert!(adapter
        .send(&id, "must not reach the killed child")
        .is_err());
}

#[test]
fn failed_mapping_persistence_prevents_process_launch_and_rolls_back_temp_file() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    for stage in [
        MappingFaultStage::Serialize,
        MappingFaultStage::FileSync,
        MappingFaultStage::Rename,
        MappingFaultStage::DirectorySync,
    ] {
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        adapter.next_mapping_fault = Some(MappingFault {
            stage,
            cleanup_error: (stage == MappingFaultStage::DirectorySync)
                .then(|| "injected rollback cleanup failure".into()),
        });
        let error = adapter
            .start(StartRequest {
                profile: format!("profile-{stage:?}"),
                instruction: "must never launch".into(),
            })
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("injected"));
        if stage == MappingFaultStage::DirectorySync {
            assert!(message.contains("injected rollback cleanup failure"));
        }
        assert!(adapter.sessions.is_empty());
        assert_eq!(fs::read_dir(&session_dir).unwrap().count(), 0);
    }
    assert!(!f.record_dir.join("argv.jsonl").exists());
}

#[test]
fn failed_initial_delivery_retires_child_and_rolls_back_durable_mapping() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for stage in [
        WriteFaultStage::Json,
        WriteFaultStage::Newline,
        WriteFaultStage::Flush,
    ] {
        let session_dir = f.record_dir.join(format!("sessions-initial-{stage:?}"));
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        adapter.next_write_fault = Some(stage);
        adapter.next_cleanup_error = Some("injected surviving child".into());
        let error = adapter
            .start(StartRequest {
                profile: format!("profile-{stage:?}"),
                instruction: "must not remain active".into(),
            })
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(&format!("injected {stage:?} write failure")));
        assert!(message.contains("injected surviving child"));
        assert!(adapter.sessions.is_empty());
        assert_eq!(fs::read_dir(session_dir).unwrap().count(), 0);
    }
}

#[test]
fn missing_child_stdio_reaps_spawned_process_and_rolls_back_mapping() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for fault in [SpawnFault::MissingStdin, SpawnFault::MissingStdout] {
        let session_dir = f.record_dir.join(format!("sessions-{fault:?}"));
        let mut adapter =
            ClaudeManagerSession::new_with_session_dir(f.bin_dir.join("claude"), &session_dir)
                .unwrap();
        adapter.next_spawn_fault = Some(fault);
        adapter.next_cleanup_error = Some("injected spawn survivor".into());
        adapter.next_wait_error = Some("injected spawn wait failure".into());
        let error = adapter
            .start(StartRequest {
                profile: format!("profile-{fault:?}"),
                instruction: "must never send".into(),
            })
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("did not provide"));
        assert!(message.contains("injected spawn survivor"));
        assert!(message.contains("injected spawn wait failure"));
        assert!(adapter.sessions.is_empty());
        assert_eq!(fs::read_dir(session_dir).unwrap().count(), 0);
    }
}

#[test]
fn send_write_failures_are_terminal_and_retire_the_transport() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for stage in [
        WriteFaultStage::Json,
        WriteFaultStage::Newline,
        WriteFaultStage::Flush,
    ] {
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join(format!("sessions-{stage:?}")),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: format!("profile-{stage:?}"),
                instruction: "complete first".into(),
            })
            .unwrap();
        assert_eq!(
            wait_for_turn(&mut adapter, &id).1,
            TerminalStatus::Completed
        );
        adapter
            .sessions
            .get_mut(&id)
            .unwrap()
            .process
            .as_mut()
            .unwrap()
            .write_fault = Some(stage);

        let error = adapter.send(&id, "ambiguous delivery").unwrap_err();
        assert!(format!("{error:#}").contains(&format!("injected {stage:?} write failure")));
        let Some(TerminalStatus::Failed(stored)) = adapter.terminal_status(&id).unwrap() else {
            panic!("write failure must become the stored terminal state");
        };
        assert!(stored.contains(&format!("injected {stage:?} write failure")));
        assert!(adapter.sessions.get(&id).unwrap().process.is_none());
    }
}

#[test]
fn failed_recovery_write_replaces_the_stored_failure_and_retires_the_transport() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for stage in [
        WriteFaultStage::Json,
        WriteFaultStage::Newline,
        WriteFaultStage::Flush,
    ] {
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join(format!("failed-recovery-{stage:?}")),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: format!("failed-recovery-{stage:?}"),
                instruction: "fail".into(),
            })
            .unwrap();
        assert!(matches!(
            wait_for_terminal_status(&mut adapter, &id),
            TerminalStatus::Failed(_)
        ));
        let process = adapter
            .sessions
            .get_mut(&id)
            .unwrap()
            .process
            .as_mut()
            .unwrap();
        process.write_fault = Some(stage);
        process.child.injected_cleanup_error = Some("injected recovery survivor".into());

        let returned = adapter
            .send(&id, "attempt recovery")
            .unwrap_err()
            .to_string();
        let Some(TerminalStatus::Failed(stored)) = adapter.terminal_status(&id).unwrap() else {
            panic!("failed recovery write must remain terminal");
        };
        assert_eq!(stored, returned);
        assert!(stored.contains(&format!("injected {stage:?} write failure")));
        assert!(stored.contains("injected recovery survivor"));
        assert!(adapter.sessions.get(&id).unwrap().process.is_none());
    }
}

#[test]
fn completed_transport_eof_is_retired_before_a_later_send() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "complete-then-close".into(),
        })
        .unwrap();
    assert_eq!(
        wait_for_turn(&mut adapter, &id).1,
        TerminalStatus::Completed
    );
    for _ in 0..100 {
        let _ = adapter.stream(&id);
        if adapter.sessions.get(&id).unwrap().process.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(adapter.sessions.get(&id).unwrap().process.is_none());

    assert!(adapter.send(&id, "must require resume").is_err());
    assert!(matches!(
        adapter.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Failed(_))
    ));
}

#[test]
fn interrupt_kills_the_child_process_and_sticks_terminated_interrupted() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let sentinel = f.record_dir.join("slow-completed.marker");
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();

    // Give the fake backend a moment to actually enter its sleep before
    // interrupting, so this proves a real in-progress turn was killed
    // rather than racing a turn that never started.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(adapter.terminal_status(&id).unwrap(), None);

    adapter.interrupt(&id).unwrap();
    assert_eq!(
        adapter.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Interrupted)
    );

    // The fake backend only writes this sentinel after its multi-second
    // sleep completes. If interrupt merely flipped a status flag without
    // killing the process, the sentinel would still appear once that
    // sleep elapses.
    std::thread::sleep(Duration::from_millis(3_200));
    assert!(
        !sentinel.exists(),
        "interrupt must terminate the real child process, not just relabel status"
    );
}

#[test]
fn queued_turn_failure_remains_sticky_after_a_later_successful_turn() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "delayed-fail".into(),
        })
        .unwrap();
    // Steer/queue a second turn before the first turn's failure has
    // necessarily been observed -- this must not let the second turn's
    // later success overwrite the first turn's failure.
    adapter.send(&id, "should still be sticky failed").unwrap();

    let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
        panic!("expected the queued turn's failure to remain the sticky terminal status");
    };
    assert!(message.contains("structured protocol failure"));

    // Draining further updates and re-polling must not flip this back
    // to Completed once the second (successful) turn's result arrives.
    for _ in 0..20 {
        let _ = adapter.stream(&id);
        assert_eq!(
            adapter.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Failed(message.clone()))
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn explicit_send_after_observed_failure_starts_a_new_working_lifecycle() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "fail".into(),
        })
        .unwrap();
    assert!(matches!(
        wait_for_terminal_status(&mut adapter, &id),
        TerminalStatus::Failed(_)
    ));

    adapter.send(&id, "recovered").unwrap();
    assert_eq!(adapter.terminal_status(&id).unwrap(), None);
    let (updates, status) = wait_for_turn(&mut adapter, &id);
    assert_eq!(status, TerminalStatus::Completed);
    assert!(updates.contains(&SessionUpdate::MessageChunk("reply: recovered".into())));
}

#[test]
fn legitimate_tool_events_are_ignored_without_changing_the_turn() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "tool-events".into(),
        })
        .unwrap();

    let (updates, status) = wait_for_turn(&mut adapter, &id);
    assert_eq!(status, TerminalStatus::Completed);
    assert_eq!(
        updates,
        vec![
            SessionUpdate::MessageChunk("reply: tool-events".into()),
            SessionUpdate::Usage {
                used: 19,
                size: 200_000,
            },
        ]
    );
}

#[test]
fn malformed_tool_events_fail_instead_of_being_silently_ignored() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for (prompt, expected) in [
        ("malformed-tool-progress", "elapsed_time_seconds"),
        ("malformed-tool-summary", "preceding_tool_use_ids"),
    ] {
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join(format!("sessions-{prompt}")),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: format!("profile-{prompt}"),
                instruction: prompt.into(),
            })
            .unwrap();
        let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
            panic!("malformed {prompt} event must fail the session");
        };
        assert!(message.contains(expected), "unexpected failure: {message}");
        assert!(adapter.sessions.get(&id).unwrap().process.is_none());
    }
}

#[test]
fn duplicate_result_without_an_outstanding_turn_fails_and_retires_the_transport() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "duplicate-result".into(),
        })
        .unwrap();

    let returned = (0..300)
        .find_map(|_| match adapter.stream(&id) {
            Err(error) => Some(error.to_string()),
            Ok(_) => {
                std::thread::sleep(Duration::from_millis(10));
                None
            }
        })
        .expect("the duplicate result must fail the session");
    assert!(returned.contains("without an outstanding turn"));
    let Some(TerminalStatus::Failed(stored)) = adapter.terminal_status(&id).unwrap() else {
        panic!("the duplicate result failure must be stored");
    };
    assert_eq!(stored, returned);
    assert!(adapter.sessions.get(&id).unwrap().process.is_none());

    assert!(adapter.send(&id, "must require resume").is_err());
    assert_eq!(
        adapter.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Failed(stored))
    );
}

#[test]
fn malformed_protocol_line_terminates_the_child_and_stays_sticky_failed() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "malformed".into(),
        })
        .unwrap();

    let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
        panic!("expected the malformed protocol line to fail the session");
    };
    assert!(message.contains("parsing Claude stream-json event"));

    // A well-formed success result was queued right behind the
    // malformed line on the wire; it must never be allowed to surface,
    // whether because the reader stopped forwarding after the bad line
    // or because a sticky-failed session ignores later terminal events.
    for _ in 0..20 {
        let _ = adapter.stream(&id);
        assert_eq!(
            adapter.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Failed(message.clone())),
            "must never observe the trailing well-formed success result"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn invalid_lifecycle_messages_fail_instead_of_leaving_work_stuck() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    for (prompt, expected) in [
        ("missing-session", "missing session_id"),
        ("malformed-assistant", "assistant message.content"),
        ("malformed-result", "result is_error"),
        ("malformed-stream", "stream event delta"),
        ("unknown-event", "unknown Claude message type"),
    ] {
        let mut adapter = ClaudeManagerSession::new_with_session_dir(
            f.bin_dir.join("claude"),
            f.record_dir.join(format!("sessions-{prompt}")),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: format!("profile-{prompt}"),
                instruction: prompt.into(),
            })
            .unwrap();
        let TerminalStatus::Failed(message) = wait_for_terminal_status(&mut adapter, &id) else {
            panic!("invalid {prompt} event must fail the session");
        };
        assert!(message.contains(expected), "unexpected failure: {message}");
        assert!(adapter.sessions.get(&id).unwrap().process.is_none());
    }
}

#[test]
fn mismatched_session_event_terminates_the_child_and_stays_failed() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_streaming_claude(&f.bin_dir, &f.record_dir);
    let mut adapter = ClaudeManagerSession::new_with_session_dir(
        f.bin_dir.join("claude"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = adapter
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "mismatched-session".into(),
        })
        .unwrap();
    adapter
        .sessions
        .get_mut(&id)
        .unwrap()
        .process
        .as_mut()
        .unwrap()
        .child
        .injected_cleanup_error = Some("injected mismatch survivor".into());

    let returned = (0..300)
        .find_map(|_| match adapter.stream(&id) {
            Err(error) => Some(error.to_string()),
            Ok(_) => {
                std::thread::sleep(Duration::from_millis(10));
                None
            }
        })
        .expect("expected the mismatched session event to fail the session");
    let Some(TerminalStatus::Failed(message)) = adapter.terminal_status(&id).unwrap() else {
        panic!("expected the combined failure to be stored");
    };
    assert!(message.contains("returned session ID"));
    assert!(message.contains("injected mismatch survivor"));
    assert_eq!(message, returned);
    assert!(adapter.send(&id, "must not reach a dead child").is_err());
    assert_eq!(
        adapter.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Failed(message))
    );
}

#[test]
fn installed_claude_start_and_restart_resume_smoke_when_requested() {
    let Some(executable) = std::env::var_os("GAH_TEST_REAL_CLAUDE") else {
        return;
    };
    let _exec_guard = crate::test_support::ExecGuard::new();
    let state = tempfile::TempDir::new().unwrap();
    let session_dir = state.path().join("manager-sessions");
    let smoke_args = vec![
        OsString::from("--safe-mode"),
        OsString::from("--tools"),
        OsString::new(),
        OsString::from("--max-budget-usd"),
        OsString::from("0.02"),
    ];
    let id = {
        let mut adapter = ClaudeManagerSession::new_with_session_dir_and_args(
            &executable,
            &session_dir,
            smoke_args.clone(),
        )
        .unwrap();
        let id = adapter
            .start(StartRequest {
                profile: "installed-smoke".into(),
                instruction: "Reply with exactly GAH_CLAUDE_START_OK".into(),
            })
            .unwrap();
        assert_bounded_real_turn(&mut adapter, &id, "GAH_CLAUDE_START_OK");
        id
    };

    let mut restarted =
        ClaudeManagerSession::new_with_session_dir_and_args(executable, session_dir, smoke_args)
            .unwrap();
    restarted.resume(&id).unwrap();
    restarted
        .send(&id, "Reply with exactly GAH_CLAUDE_RESUME_OK")
        .unwrap();
    assert_bounded_real_turn(&mut restarted, &id, "GAH_CLAUDE_RESUME_OK");
}

fn assert_bounded_real_turn(
    adapter: &mut ClaudeManagerSession,
    id: &GahSessionId,
    expected_reply: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut updates = Vec::new();
    while Instant::now() < deadline {
        updates.extend(adapter.stream(id).unwrap());
        if let Some(status) = adapter.terminal_status(id).unwrap() {
            updates.extend(adapter.stream(id).unwrap());
            assert_eq!(status, TerminalStatus::Completed);
            let reply = updates
                .iter()
                .filter_map(|update| match update {
                    SessionUpdate::MessageChunk(text) => Some(text.as_str()),
                    SessionUpdate::Usage { .. } => None,
                })
                .collect::<String>();
            assert_eq!(reply.trim(), expected_reply);
            assert!(
                updates
                    .iter()
                    .any(|update| matches!(update, SessionUpdate::Usage { .. })),
                "installed Claude success omitted structured usage"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("installed Claude protocol smoke exceeded its 30-second bound");
}
