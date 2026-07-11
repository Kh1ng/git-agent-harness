//! Claude Code quota / usage monitoring (issue #153).
//!
//! Three independent, well-tested pieces:
//!
//! 1. **Per-attempt usage from the session transcript JSON** — Claude Code
//!    writes a JSONL transcript per session (`~/.claude/projects/.../*.jsonl`).
//!    Assistant turns carry a `message.usage` object with real token counts
//!    (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
//!    `cache_read_input_tokens`) and a `cost_usd` rollup. We never scrape
//!    stdout for this — we parse the structured transcript. A Stop hook
//!    receives the transcript path on stdin, so the same parser backs the
//!    Stop-hook path by reading that file.
//!
//! 2. **Live context from the status line** — Claude Code's status-line
//!    feature emits a JSON document with the live session state (model, tokens
//!    used / limit, running cost, turn count, uptime). [`parse_claude_status_line`]
//!    decodes it into a typed [`ClaudeStatusLine`].
//!
//! 3. **Quota via PTY-captured `/usage`** — `/usage` is an *interactive*
//!    slash command, not a CLI flag, so it must run inside a real
//!    pseudo-terminal. [`capture_usage_via_pty`] opens a PTY, spawns `claude`,
//!    types `/usage`, and captures the rendered table which
//!    [`parse_claude_usage_text`] then parses. This is intentionally a
//!    standalone, dependency-light module so the novel PTY plumbing is
//!    isolated and unit-tested with a fake `claude` binary.

use crate::ledger::LedgerUsage;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Well-known home-relative location of Claude Code transcript projects.
pub const CLAUDE_PROJECTS_DIR: &str = ".claude/projects";

// ---------------------------------------------------------------------------
// Transcript / Stop-hook per-attempt usage
// ---------------------------------------------------------------------------

/// Parse a Claude Code session transcript (JSONL) and aggregate the real
/// per-attempt token/cost usage from every assistant turn's `message.usage`.
///
/// Returns a default [`LedgerUsage`] (no `usage_source`) when the input
/// contains no decodable usage, so callers can fall back to other parsers.
pub fn parse_claude_transcript_usage(transcript_jsonl: &str) -> LedgerUsage {
    let mut usage = LedgerUsage::default();
    let mut total_cost = 0.0f64;
    let mut saw_any = false;

    for line in transcript_jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // Only assistant turns carry usage.
        if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(u) = message.get("usage") else {
            continue;
        };
        saw_any = true;
        if usage.actual_model.is_none() {
            usage.actual_model = message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string);
        }

        let get = |k: &str| u.get(k).and_then(|v| v.as_u64());
        usage.input_tokens =
            Some(usage.input_tokens.unwrap_or(0) + get("input_tokens").unwrap_or(0));
        usage.output_tokens =
            Some(usage.output_tokens.unwrap_or(0) + get("output_tokens").unwrap_or(0));
        usage.cache_write_tokens = Some(
            usage.cache_write_tokens.unwrap_or(0) + get("cache_creation_input_tokens").unwrap_or(0),
        );
        usage.cache_read_tokens = Some(
            usage.cache_read_tokens.unwrap_or(0) + get("cache_read_input_tokens").unwrap_or(0),
        );
        usage.total_tokens = Some(
            usage.total_tokens.unwrap_or(0)
                + get("input_tokens").unwrap_or(0)
                + get("output_tokens").unwrap_or(0),
        );
        if let Some(c) = value.get("cost_usd").and_then(|v| v.as_f64()) {
            total_cost += c;
        }
    }

    if !saw_any {
        return usage;
    }
    usage.usage_source = Some("claude_transcript_json".to_string());
    if total_cost > 0.0 {
        usage.actual_cost_usd = Some((total_cost * 1e6).round() / 1e6);
    }
    usage
}

/// Parse the JSON payload a Claude Stop hook receives on stdin (which names the
/// transcript file) and return the transcript's parsed usage. Returns a default
/// `LedgerUsage` when the payload is missing or the transcript is unreadable.
pub fn usage_from_stop_hook_payload(json: &str) -> LedgerUsage {
    let Ok(payload) = serde_json::from_str::<Value>(json) else {
        return LedgerUsage::default();
    };
    let Some(transcript_path) = payload.get("transcript_path").and_then(|v| v.as_str()) else {
        return LedgerUsage::default();
    };
    let Ok(text) = std::fs::read_to_string(transcript_path) else {
        return LedgerUsage::default();
    };
    parse_claude_transcript_usage(&text)
}

/// Resolve the on-disk transcript path for a given Claude session id, rooted at
/// the given home directory (or `$HOME` when `None`). Returns `None` if the
/// projects directory (or the session file) cannot be located — callers then
/// fall back to the stdout parser.
pub fn transcript_path_for_session(
    session_id: &str,
    home: Option<&str>,
) -> Option<std::path::PathBuf> {
    let home = home
        .map(|s| s.to_string())
        .or_else(|| std::env::var("HOME").ok())?;
    let projects = Path::new(&home).join(CLAUDE_PROJECTS_DIR);
    if !projects.is_dir() {
        return None;
    }
    // Claude stores transcripts under per-project subdirectories, named
    // `<session-id>.jsonl`. Search recursively for the first match.
    let mut stack = vec![projects];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == format!("{session_id}.jsonl"))
                .unwrap_or(false)
            {
                return Some(path);
            }
        }
    }
    None
}

/// Convenience wrapper used by the runner: locate the transcript produced for a
/// pinned `session_id`, rooted at a specific home directory path. The
/// `worktree` argument is accepted for API symmetry (Claude nests transcripts
/// under the project path derived from the cwd) but the recursive search above
/// already covers the projects tree, so it is not required for the lookup.
pub fn find_claude_transcript(
    home: &Path,
    _worktree: &Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    transcript_path_for_session(session_id, home.to_str())
}

// ---------------------------------------------------------------------------
// Status-line live context
// ---------------------------------------------------------------------------

/// Decoded Claude Code status-line JSON (live session context).
///
/// Field names follow Claude Code's documented status-line schema. Unknown or
/// absent fields are left `None` rather than fabricated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeStatusLine {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub cwd: Option<String>,
    pub tokens_used: Option<u64>,
    pub tokens_limit: Option<u64>,
    pub cost_usd: Option<f64>,
    pub turns: Option<u64>,
    pub duration_ms: Option<u64>,
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusLineRaw {
    #[serde(rename = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "model")]
    model: Option<String>,
    #[serde(rename = "status")]
    status: Option<String>,
    #[serde(rename = "cwd")]
    cwd: Option<String>,
    #[serde(rename = "tokens_used")]
    tokens_used: Option<u64>,
    #[serde(rename = "tokens_limit")]
    tokens_limit: Option<u64>,
    #[serde(rename = "cost_usd")]
    cost_usd: Option<f64>,
    #[serde(rename = "turns")]
    turns: Option<u64>,
    #[serde(rename = "duration_ms")]
    duration_ms: Option<u64>,
    #[serde(rename = "version")]
    version: Option<String>,
}

/// Parse the JSON document Claude Code's status-line feature emits. Returns
/// `None` on any parse failure so callers never fabricate live context.
pub fn parse_claude_status_line(json: &str) -> Option<ClaudeStatusLine> {
    let raw: StatusLineRaw = serde_json::from_str(json).ok()?;
    Some(ClaudeStatusLine {
        session_id: raw.session_id,
        model: raw.model,
        status: raw.status,
        cwd: raw.cwd,
        tokens_used: raw.tokens_used,
        tokens_limit: raw.tokens_limit,
        cost_usd: raw.cost_usd,
        turns: raw.turns,
        duration_ms: raw.duration_ms,
        version: raw.version,
    })
}

// ---------------------------------------------------------------------------
// PTY-captured /usage quota
// ---------------------------------------------------------------------------

/// Result of a PTY-backed `/usage` capture.
#[derive(Debug, Clone)]
pub struct UsageCapture {
    /// Raw string captured from the PTY (ready to feed to
    /// [`parse_claude_usage_text`]).
    pub raw: String,
    /// Whether the interactive session reached a usable prompt before we sent
    /// `/usage` (best-effort liveness signal).
    pub reached_prompt: bool,
}

/// Parse the human-readable output of Claude Code's `/usage` command (the
/// rendered "Token usage by model" table and "Total cost" line). Returns a
/// `LedgerUsage` whose `usage_source` is `Some("claude_usage_text")` only when
/// a cost or per-model total was actually found. Per-model token counts are
/// aggregated into the top-level token fields.
pub fn parse_claude_usage_text(text: &str) -> LedgerUsage {
    let mut usage = LedgerUsage::default();
    let mut saw_anything = false;

    for line in text.lines() {
        let line = line.trim();
        // "Total cost:                $0.0123"
        if let Some(rest) = line.strip_prefix("Total cost") {
            if let Some(cost) = parse_dollar(rest) {
                usage.actual_cost_usd = Some((cost * 1e6).round() / 1e6);
                saw_anything = true;
            }
        }
        // "  claude-sonnet-4-20250514: 12,345 input + 678 output (0 cache creation, 12,345 cache read) = 25,368 total"
        if let Some((model, rest)) = line.split_once(':') {
            let model = model.trim();
            if model.is_empty() || model.parse::<u64>().is_ok() {
                continue;
            }
            let input = find_num_before(rest, "input");
            let output = find_num_before(rest, "output");
            let cache_read = find_num_before(rest, "cache read");
            let cache_creation = find_num_before(rest, "cache creation");
            if input.is_some()
                || output.is_some()
                || cache_read.is_some()
                || cache_creation.is_some()
            {
                usage.input_tokens = Some(usage.input_tokens.unwrap_or(0) + input.unwrap_or(0));
                usage.output_tokens = Some(usage.output_tokens.unwrap_or(0) + output.unwrap_or(0));
                usage.cache_read_tokens =
                    Some(usage.cache_read_tokens.unwrap_or(0) + cache_read.unwrap_or(0));
                usage.cache_write_tokens =
                    Some(usage.cache_write_tokens.unwrap_or(0) + cache_creation.unwrap_or(0));
                usage.total_tokens = Some(
                    usage.total_tokens.unwrap_or(0) + input.unwrap_or(0) + output.unwrap_or(0),
                );
                saw_anything = true;
            }
        }
    }

    if saw_anything {
        usage.usage_source = Some("claude_usage_text".to_string());
    }
    usage
}

fn parse_dollar(s: &str) -> Option<f64> {
    let dollar = s.find('$')?;
    let num: String = s[dollar + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    num.replace(',', "").parse::<f64>().ok()
}

fn find_num_before(haystack: &str, keyword: &str) -> Option<u64> {
    let idx = haystack.find(keyword)?;
    let before = &haystack[..idx];
    let bytes: Vec<char> = before.chars().rev().collect();
    let mut num = String::new();
    for c in bytes {
        if c.is_ascii_digit() || c == ',' {
            num.push(c);
        } else if !num.is_empty() {
            break;
        }
    }
    if num.is_empty() {
        return None;
    }
    num.replace(',', "")
        .chars()
        .rev()
        .collect::<String>()
        .parse::<u64>()
        .ok()
}

/// Spawn `claude` inside a real pseudo-terminal, drive the interactive `/usage`
/// slash command, and capture the rendered quota/usage table.
///
/// `/usage` is an interactive slash command (not a CLI flag), so it must be run
/// inside a pseudo-terminal. We rely on the system `script(1)` utility
/// (util-linux) to own the PTY/session/foreground-group plumbing, which is the
/// robust, well-tested way to allocate a controlling terminal for a child.
/// Driving the raw `openpty`/`setsid`/`TIOCSCTTY`/`tcsetpgrp` dance by hand is
/// fragile: in many container environments the child's `read(stdin)` is stopped
/// by SIGTTIN even after `tcsetpgrp`, so the slash command never receives input.
///
/// The protocol is:
///   1. Launch `script -qec "<claude_path> /usage" <typescript>`, which connects
///      a PTY to `claude` (running the `/usage` command on startup).
///   2. Parse the captured typescript for the usage table.
///
/// Returns an error if `script(1)` is unavailable or the session cannot start.
pub fn capture_usage_via_pty(
    claude_path: &str,
    timeout: Option<Duration>,
) -> std::io::Result<UsageCapture> {
    let timeout = timeout.unwrap_or(Duration::from_secs(30));

    // Locate `script(1)`. This is a hard requirement for the PTY path; if it is
    // missing we surface an explicit error rather than silently degrading.
    let script_bin = which("script");

    // Unique typescript per call so concurrent `capture_usage_via_pty`
    // invocations (e.g. parallel tests) never clobber each other's file.
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let typescript = std::env::temp_dir().join(format!(
        "claude_usage_{}_{}_{}.log",
        std::process::id(),
        nonce,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&typescript);

    let status = match script_bin {
        Some(script) => {
            let cmd = format!("{} /usage", claude_path);
            let mut command = std::process::Command::new(script);
            #[cfg(target_os = "macos")]
            command
                .args(["-q", typescript.to_str().unwrap_or("")])
                .args(["sh", "-c", &cmd]);
            #[cfg(not(target_os = "macos"))]
            command.args(["-qec", &cmd]).arg(&typescript);
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
        }
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the `script` utility (util-linux) is required to allocate a PTY for `claude /usage`",
            ));
        }
    };

    // A non-zero `script` exit (e.g. 127 when the `claude` binary is missing,
    // or a non-zero code the inner command returns) is a real failure of the
    // PTY capture and must surface as an error rather than fabricating empty
    // usage. We read the typescript regardless, but only when the session
    // actually ran.
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let _ = std::fs::remove_file(&typescript);
            return Err(std::io::Error::other(format!(
                "`script` reported a failed `claude /usage` session: exit status {s}"
            )));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&typescript);
            return Err(e);
        }
    }

    // `script` (and the inner `claude`) flush their output to the typescript
    // asynchronously. Rather than a fixed sleep, poll briefly for the file to
    // appear and stabilise, bounded by the overall timeout.
    let deadline = std::time::Instant::now() + timeout;
    let mut stable_for = Duration::from_secs(0);
    let mut last_len = 0usize;
    loop {
        if let Ok(meta) = std::fs::metadata(&typescript) {
            if meta.len() > 0 {
                let len = meta.len() as usize;
                if len == last_len {
                    stable_for += Duration::from_millis(20);
                } else {
                    stable_for = Duration::from_secs(0);
                    last_len = len;
                }
                // Treat the file as ready once it has held steady for ~40ms
                // (long enough for the final flush) or we've run out of time.
                if stable_for >= Duration::from_millis(40) || std::time::Instant::now() >= deadline
                {
                    break;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let raw = std::fs::read_to_string(&typescript).unwrap_or_default();
    let _ = std::fs::remove_file(&typescript);

    // Did we actually see a Claude prompt, or did the session fail to start?
    let reached_prompt = raw.contains('>') || raw.contains("claude") || raw.contains("Welcome");

    Ok(UsageCapture {
        raw,
        reached_prompt,
    })
}

/// Minimal `which` that searches `$PATH` for an executable file. We avoid a
/// dependency on a crate by using the libc `access(2)` check.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        let c = std::ffi::CString::new(candidate.as_os_str().to_string_lossy().as_bytes()).ok()?;
        // X_OK: file exists and is executable.
        let ok = unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0;
        if ok {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = r#"
{"type":"system","sessionId":"sess-1"}
{"type":"assistant","message":{"id":"t1","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":2100,"output_tokens":420,"cache_creation_input_tokens":80,"cache_read_input_tokens":9900}},"cost_usd":0.0081}
{"type":"user","message":{"role":"user","content":"go"}}
{"type":"assistant","message":{"id":"t2","role":"assistant","model":"claude-sonnet-4-20250514","usage":{"input_tokens":640,"output_tokens":150,"cache_creation_input_tokens":0,"cache_read_input_tokens":3300}},"cost_usd":0.0024}
{"type":"assistant","message":{"id":"t3","role":"assistant","model":"claude-sonnet-4-20250514","usage":{}}}
"#;

    #[test]
    fn transcript_sums_turns() {
        let u = parse_claude_transcript_usage(TRANSCRIPT);
        assert_eq!(u.input_tokens, Some(2740));
        assert_eq!(u.output_tokens, Some(570));
        assert_eq!(u.cache_write_tokens, Some(80));
        assert_eq!(u.cache_read_tokens, Some(13200));
        assert_eq!(u.total_tokens, Some(3310));
        assert_eq!(u.actual_cost_usd, Some(0.0105));
        assert_eq!(u.actual_model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(u.usage_source.as_deref(), Some("claude_transcript_json"));
    }

    #[test]
    fn transcript_empty_when_no_usage() {
        let u = parse_claude_transcript_usage(
            "{\"type\":\"system\",\"sessionId\":\"x\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n",
        );
        assert!(u.usage_source.is_none());
        assert_eq!(u.input_tokens, None);
    }

    const USAGE_TEXT: &str = "Token usage by model
  claude-sonnet-4-20250514: 12,345 input + 678 output (0 cache creation, 12,345 cache read) = 25,368 total
  claude-opus-4-20250514: 1,000 input + 200 output (10 cache creation, 0 cache read) = 1,210 total

Total cost:                $0.0123
";

    #[test]
    fn usage_text_parses_per_model_and_cost() {
        let u = parse_claude_usage_text(USAGE_TEXT);
        assert_eq!(u.actual_cost_usd, Some(0.0123));
        assert_eq!(u.usage_source.as_deref(), Some("claude_usage_text"));
        // The two models' tokens are aggregated into the top-level fields.
        assert_eq!(u.input_tokens, Some(13_345));
        assert_eq!(u.output_tokens, Some(878));
        assert_eq!(u.cache_read_tokens, Some(12_345));
        assert_eq!(u.cache_write_tokens, Some(10));
        assert_eq!(u.total_tokens, Some(14_223));
    }

    #[test]
    fn usage_text_empty_on_noise() {
        let u = parse_claude_usage_text("nothing about tokens here");
        assert!(u.usage_source.is_none());
        assert_eq!(u.actual_cost_usd, None);
    }

    const STATUS: &str = r#"{"session_id":"sess-1","model":"claude-opus-4-20250514","status":"idle","cwd":"/repo","tokens_used":50000,"tokens_limit":200000,"cost_usd":0.02,"turns":7,"duration_ms":12345,"version":"1.0.16"}"#;

    #[test]
    fn status_line_decodes() {
        let s = parse_claude_status_line(STATUS).unwrap();
        assert_eq!(s.session_id.as_deref(), Some("sess-1"));
        assert_eq!(s.model.as_deref(), Some("claude-opus-4-20250514"));
        assert_eq!(s.status.as_deref(), Some("idle"));
        assert_eq!(s.tokens_used, Some(50_000));
        assert_eq!(s.tokens_limit, Some(200_000));
        assert_eq!(s.cost_usd, Some(0.02));
        assert_eq!(s.turns, Some(7));
        assert_eq!(s.duration_ms, Some(12_345));
    }

    #[test]
    fn status_line_none_on_garbage() {
        assert!(parse_claude_status_line("not json").is_none());
    }

    // The PTY path is the novel/risky piece (issue #153). Back it with a fake
    // `claude` binary that, when invoked as `<fake> /usage` (exactly how
    // `capture_usage_via_pty` launches it through `script(1)`), renders the
    // interactive quota table straight to its own stdout (which `script`
    // captures into the typescript).
    #[test]
    fn pty_captures_usage_from_fake_claude() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("gah_claude_fake_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fake = dir.join("claude.sh");
        let script = "#!/bin/sh\n\
printf 'Welcome to Claude Code (fake)\\n\\n> \\n'\n\
printf '\\nToken usage by model\\n'\n\
printf '  claude-sonnet-4-20250514: 12,345 input + 678 output (0 cache creation, 12,345 cache read) = 25,368 total\\n'\n\
printf '\\nTotal cost:                $0.0123\\n'\n\
printf '\\n> \\n'\n";
        let mut f = std::fs::File::create(&fake).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }

        let capture = capture_usage_via_pty(
            fake.to_str().unwrap(),
            Some(std::time::Duration::from_secs(10)),
        )
        .expect("pty capture should succeed against fake claude");

        let usage = parse_claude_usage_text(&capture.raw);
        assert_eq!(usage.usage_source.as_deref(), Some("claude_usage_text"));
        assert_eq!(usage.actual_cost_usd, Some(0.0123));
        assert_eq!(usage.input_tokens, Some(12_345));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
