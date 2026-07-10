//! Issue #153: Claude quota monitoring — per-attempt usage (transcript/Stop hook),
//! PTY `/usage` capture, and status-line parsing.
//!
//! These tests exercise the real parsers in `gah::claude_monitor` against committed
//! fixtures under `tests/fixtures/claude/`, plus a fake `claude` binary wrapped in a
//! PTY to validate the interactive `/usage` capture path end-to-end.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use git_agent_harness::claude_monitor::*;

/// Minimal single-argument POSIX shell quoting for embedding fixture text in
/// the fake `claude` script without breaking the `printf` statement.
fn shell_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(name);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read fixture {p:?}: {e}"))
}

#[test]
fn transcript_parser_sums_per_turn_tokens() {
    let text = fixture("transcript.jsonl");
    let usage = parse_claude_transcript_usage(&text);

    // turn-1 + turn-2, turn-3 has empty usage.
    assert_eq!(usage.input_tokens, Some(2740));
    assert_eq!(usage.output_tokens, Some(570));
    assert_eq!(usage.cache_write_tokens, Some(80));
    assert_eq!(usage.cache_read_tokens, Some(13200));
    assert_eq!(usage.total_tokens, Some(3310));
    // cost_usd: 0.0081 + 0.0024
    assert_eq!(usage.actual_cost_usd, Some(0.0105));
    assert_eq!(
        usage.usage_source.as_deref(),
        Some("claude_transcript_json")
    );
}

#[test]
fn transcript_parser_handles_empty_and_missing_usage() {
    // No assistant usage blocks at all -> no observation.
    let text = "{\"type\":\"system\",\"sessionId\":\"x\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n";
    let usage = parse_claude_transcript_usage(text);
    assert!(usage.usage_source.is_none());
    assert_eq!(usage.input_tokens, None);
}

#[test]
fn usage_text_parser_extracts_totals_and_cost() {
    let text = fixture("usage.txt");
    let usage = parse_claude_usage_text(&text);

    // Final "Total cost" line is captured as cost.
    assert_eq!(usage.actual_cost_usd, Some(0.0123));
    assert_eq!(usage.usage_source.as_deref(), Some("claude_usage_text"));
    // Per-model tokens are aggregated into the top-level token fields.
    assert_eq!(usage.input_tokens, Some(13_345));
    assert_eq!(usage.output_tokens, Some(878));
}

#[test]
fn usage_text_parser_returns_empty_on_unrelated_text() {
    let usage = parse_claude_usage_text("nothing about tokens here");
    assert!(usage.usage_source.is_none());
    assert_eq!(usage.actual_cost_usd, None);
}

#[test]
fn status_line_parser_reads_live_context() {
    let text = fixture("status-line.json");
    let status = parse_claude_status_line(&text).expect("valid status-line JSON");

    assert_eq!(status.session_id.as_deref(), Some("sess-fixture-1"));
    assert_eq!(status.model.as_deref(), Some("claude-opus-4-20250514"));
    assert_eq!(status.status.as_deref(), Some("idle"));
    assert_eq!(status.tokens_used, Some(50_000));
    assert_eq!(status.tokens_limit, Some(200_000));
    assert_eq!(status.cost_usd, Some(0.02));
    assert_eq!(status.turns, Some(7));
    assert_eq!(status.duration_ms, Some(12_345));
}

#[test]
fn status_line_parser_handles_non_json_gracefully() {
    // The status-line is sometimes wrapped in terminal escape noise; the parser
    // must not panic and should return None rather than fabricate numbers.
    let status = parse_claude_status_line("not json at all");
    assert!(status.is_none());
}

// A tiny fake `claude` that, when invoked with `/usage`, writes a deterministic
// usage block to stdout (as the real binary does inside a PTY), and otherwise
// exits 0.
fn write_fake_claude(dir: &Path, usage_output: &str) {
    let bin = dir.join("claude");
    // Build a POSIX-shell fake `claude` that, when invoked as `<bin> /usage`,
    // prints the supplied usage block to stdout (the way the real binary
    // renders the interactive `/usage` table inside a PTY). We construct the
    // script via a plain string (no format! args) so the `$@` / `[ ... ]`
    // shell syntax is preserved verbatim, then splice the fixture text in.
    let mut script = String::from(
        "#!/bin/sh\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"/usage\" ]; then\n\
    printf '%s'\n\
    exit 0\n\
  fi\n\
done\n\
exit 0\n",
    );
    script = script.replace(
        "printf '%s'",
        &format!("printf '%s' {}", shell_quote(usage_output)),
    );
    fs::write(&bin, script).unwrap();
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin, perms).unwrap();
}

#[test]
fn pty_usage_capture_runs_against_fake_claude() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = PathBuf::from(dir.path()).join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let usage_block = "Token usage by model\n  claude-sonnet-4-20250514: 1,000 input + 50 output (0 cache creation, 2,000 cache read) = 1,050 total\nTotal cost:                $0.0042\n";
    write_fake_claude(&bin_dir, usage_block);

    let capture = capture_usage_via_pty(bin_dir.join("claude").to_str().unwrap(), None)
        .expect("pty capture should succeed");

    assert!(capture.raw.contains("Total cost"));
    let usage = parse_claude_usage_text(&capture.raw);
    assert_eq!(usage.actual_cost_usd, Some(0.0042));
    assert_eq!(usage.usage_source.as_deref(), Some("claude_usage_text"));
}

#[test]
fn pty_usage_capture_errors_when_binary_missing() {
    let result = capture_usage_via_pty("/nonexistent/claude", None);
    assert!(result.is_err());
}

// Ensure the fixture Stop-hook payload shape is usable to locate a transcript.
#[test]
fn stop_hook_payload_locates_transcript() {
    let payload = fixture("stop-hook-payload.json");
    let hook: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let transcript = hook
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .expect("stop hook payload carries transcript_path");
    assert!(transcript.ends_with("transcript.jsonl"));

    // The parser also works directly from the named file.
    let usage = usage_from_stop_hook_payload(&payload);
    assert_eq!(
        usage.usage_source.as_deref(),
        Some("claude_transcript_json")
    );
}
