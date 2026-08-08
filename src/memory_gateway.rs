//! Rust-side client for the TDAI memory gateway's ticket-scoped recall
//! (issue #830: "shared long-term memory across all dispatch backends, not
//! just manager chat"). Mirrors `apps/server/src/managerChat/memoryGatewayClient.ts`'s
//! `recallForTicket`/`sessionKeyForTicket` -- session keys MUST match that
//! side exactly (`gah:worker:{project_key}:{ticket_id}`) or a repair/review
//! agent and manager chat land in different memory spaces for the same
//! ticket, defeating the point of a shared store.
//!
//! **Fail open, deliberately the opposite of manager-chat's policy.**
//! `memoryGatewayClient.ts`'s own doc comment says recall/capture
//! "hard-block the turn on failure (throw) rather than degrading silently"
//! for manager chat (issue #849) -- but that same file's `recallForTicket`
//! doc comment says wiring it into dispatch is "issue #830's scope, gated
//! on #878 landing first," where #878 is specifically about manager-chat's
//! hard-block being wrong for *its* use case. A dispatch attempt has a
//! human-approved ticket and existing verification gates regardless of
//! memory; the gateway being down must never block real work from
//! starting. Every function here returns `Option`/best-effort and logs
//! (never panics/bails) on any failure -- missing env config, unreachable
//! gateway, malformed response.
//!
//! HTTP calls shell out to `curl` (config via stdin, matching
//! `src/central_claims.rs`'s established pattern for keeping a bearer
//! token out of argv/ps) rather than adding a Rust HTTP client dependency
//! -- this codebase has none today, and the dispatch pipeline is
//! synchronous throughout.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

const STATUS_MARKER: &str = "__GAH_MEMORY_GATEWAY_STATUS__:";
const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:8420";
const RECALL_TIMEOUT_SECONDS: u32 = 10;

pub trait MemoryGatewayTransport {
    /// Returns `(http_status, response_body)` on any completed HTTP
    /// exchange; `Err` only for a transport-level failure (can't connect,
    /// timeout, DNS).
    fn post(
        &self,
        url: &str,
        body: &str,
        token: Option<&str>,
        timeout_secs: u32,
    ) -> Result<(u16, Vec<u8>)>;
}

pub struct CurlMemoryGatewayTransport;

impl MemoryGatewayTransport for CurlMemoryGatewayTransport {
    fn post(
        &self,
        url: &str,
        body: &str,
        token: Option<&str>,
        timeout_secs: u32,
    ) -> Result<(u16, Vec<u8>)> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-sS",
            "--max-time",
            &timeout_secs.to_string(),
            "-K",
            "-",
            "-w",
            &format!("\n{STATUS_MARKER}%{{http_code}}\n"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        crate::runner::process::arm_child_pdeathsig(&mut cmd);
        let mut child = cmd
            .spawn()
            .context("spawning curl for memory gateway call")?;
        if let Some(mut stdin) = child.stdin.take() {
            let escaped_url = url.replace('\\', "\\\\").replace('"', "\\\"");
            let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
            let mut config = format!(
                "silent\nurl = \"{escaped_url}\"\nheader = \"Content-Type: application/json\"\ndata = \"{escaped_body}\"\n"
            );
            if let Some(t) = token {
                let escaped_token = t.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!(
                    "header = \"Authorization: Bearer {escaped_token}\"\n"
                ));
            }
            stdin.write_all(config.as_bytes())?;
        }
        let output = child.wait_with_output().context("waiting for curl")?;
        if !output.status.success() {
            anyhow::bail!(
                "memory gateway request failed (curl exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let marker_idx = stdout.rfind(STATUS_MARKER).ok_or_else(|| {
            anyhow::anyhow!("curl output missing status marker (malformed response?)")
        })?;
        let status: u16 = stdout[marker_idx + STATUS_MARKER.len()..]
            .trim()
            .parse()
            .context("parsing HTTP status from curl output")?;
        let response_body = stdout[..marker_idx].trim_end().as_bytes().to_vec();
        Ok((status, response_body))
    }
}

#[derive(Debug, Deserialize)]
struct RecallResponse {
    context: String,
    code: i64,
    #[serde(default)]
    message: String,
}

fn gateway_url() -> String {
    std::env::var("TDAI_GATEWAY_URL").unwrap_or_else(|_| DEFAULT_GATEWAY_URL.to_string())
}

fn gateway_api_key() -> Option<String> {
    std::env::var("TDAI_GATEWAY_API_KEY").ok()
}

/// Same normalization as `memoryGatewayClient.ts`'s `normalizeRemoteUrl`:
/// strips scheme, credentials, and a trailing `.git`, and converts
/// scp-style `host:path` to `host/path`. Must stay byte-for-byte
/// equivalent to the TS side -- this is the project half of the shared
/// session key.
pub(crate) fn normalize_remote_url(raw: &str) -> String {
    let mut s = raw.trim().to_lowercase();
    if let Some(idx) = s.find("://") {
        let scheme = &s[..idx];
        if scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
        {
            s = s[idx + 3..].to_string();
        }
    }
    if let Some(idx) = s.find('@') {
        if !s[..idx].contains('/') {
            s = s[idx + 1..].to_string();
        }
    }
    // scp-style host:path -> host/path, but don't touch a port (":NNN/").
    if let Some(idx) = s.find(':') {
        let after = &s[idx + 1..];
        let looks_like_port = after
            .split('/')
            .next()
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        if !looks_like_port {
            s = format!("{}/{}", &s[..idx], after);
        }
    }
    if let Some(stripped) = s.strip_suffix(".git") {
        s = stripped.to_string();
    }
    s.trim_end_matches('/').to_string()
}

/// Resolves this profile's project key by reading its git remote (same
/// approach `memoryGatewayClient.ts`'s `resolveProjectKey` takes via
/// `git remote get-url origin`), falling back to the raw profile name on
/// any failure -- not a git repo yet, no `origin` remote, git missing.
fn resolve_project_key(profile_name: &str, local_path: &str) -> String {
    let output = Command::new("git")
        .args(["-C", local_path, "remote", "get-url", "origin"])
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let url = String::from_utf8_lossy(&out.stdout);
            let trimmed = url.trim();
            if trimmed.is_empty() {
                profile_name.to_string()
            } else {
                normalize_remote_url(trimmed)
            }
        }
        _ => profile_name.to_string(),
    }
}

/// `gah:worker:{project_key}:{ticket_id}` -- the leaf scope
/// `memoryGatewayClient.ts`'s `sessionKeyForTicket` already documents as
/// the target for this wiring (issue #885 shipped the TS half).
pub(crate) fn session_key_for_ticket(
    profile_name: &str,
    local_path: &str,
    work_id: &str,
) -> String {
    let project_key = resolve_project_key(profile_name, local_path);
    let ticket_id = crate::work_claim::normalize_work_identity(work_id);
    format!("gah:worker:{project_key}:{ticket_id}")
}

/// Best-effort recall for one ticket's dispatch prompt. Returns `None`
/// (never an error) when `TDAI_GATEWAY_URL` behavior can't be satisfied
/// for any reason -- gateway unset/unreachable, non-2xx, malformed
/// response, or a genuinely empty result. Caller injects the returned text
/// as an optional prompt section; nothing downstream treats its absence as
/// a failure.
pub fn recall_for_ticket(
    profile_name: &str,
    local_path: &str,
    work_id: &str,
    query: &str,
) -> Option<String> {
    recall_for_ticket_with_transport(
        &CurlMemoryGatewayTransport,
        profile_name,
        local_path,
        work_id,
        query,
    )
}

fn recall_for_ticket_with_transport(
    transport: &dyn MemoryGatewayTransport,
    profile_name: &str,
    local_path: &str,
    work_id: &str,
    query: &str,
) -> Option<String> {
    let session_key = session_key_for_ticket(profile_name, local_path, work_id);
    let url = format!("{}/recall", gateway_url());
    let body = serde_json::json!({ "query": query, "session_key": session_key }).to_string();
    let token = gateway_api_key();

    let (status, response_body) =
        match transport.post(&url, &body, token.as_deref(), RECALL_TIMEOUT_SECONDS) {
            Ok(result) => result,
            Err(err) => {
                eprintln!(
                    "[gah] memory gateway recall failed (swallowed, dispatch continues): {err:#}"
                );
                return None;
            }
        };
    if status != 200 {
        eprintln!(
            "[gah] memory gateway recall returned status {status} (swallowed): {}",
            String::from_utf8_lossy(&response_body)
        );
        return None;
    }
    let parsed: RecallResponse = match serde_json::from_slice(&response_body) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("[gah] memory gateway recall returned unparseable JSON (swallowed): {err:#}");
            return None;
        }
    };
    if parsed.code != 0 {
        eprintln!(
            "[gah] memory gateway recall degraded (code={}, swallowed): {}",
            parsed.code, parsed.message
        );
        return None;
    }
    let trimmed = parsed.context.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn normalize_remote_url_handles_https_and_scp_and_git_suffix() {
        assert_eq!(
            normalize_remote_url("https://github.com/Kh1ng/git-agent-harness.git"),
            "github.com/kh1ng/git-agent-harness"
        );
        assert_eq!(
            normalize_remote_url("git@github.com:Kh1ng/git-agent-harness.git"),
            "github.com/kh1ng/git-agent-harness"
        );
        assert_eq!(
            normalize_remote_url("http://gitlab.example.com:8080/group/project.git"),
            "gitlab.example.com:8080/group/project",
        );
    }

    #[test]
    fn session_key_uses_normalized_project_and_ticket_id() {
        // No git repo at this path, so resolve_project_key falls back to
        // the profile name -- exercises the same fallback path production
        // hits for a fresh worktree with no remote configured yet.
        let key = session_key_for_ticket("gah", "/nonexistent/path/for/test", "TICKET-0362");
        assert_eq!(key, "gah:worker:gah:#362");
    }

    type FakeResponse = std::result::Result<(u16, Vec<u8>), String>;

    struct FakeTransport {
        responses: RefCell<std::collections::VecDeque<FakeResponse>>,
        calls: RefCell<Vec<(String, String, Option<String>)>>,
    }

    impl FakeTransport {
        fn queue(responses: Vec<(u16, &str)>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(s, b)| Ok((s, b.as_bytes().to_vec())))
                        .collect(),
                ),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl MemoryGatewayTransport for FakeTransport {
        fn post(
            &self,
            url: &str,
            body: &str,
            token: Option<&str>,
            _timeout_secs: u32,
        ) -> Result<(u16, Vec<u8>)> {
            self.calls.borrow_mut().push((
                url.to_string(),
                body.to_string(),
                token.map(String::from),
            ));
            match self.responses.borrow_mut().pop_front() {
                Some(Ok(r)) => Ok(r),
                Some(Err(e)) => anyhow::bail!("{e}"),
                None => anyhow::bail!("FakeTransport: no more queued responses"),
            }
        }
    }

    #[test]
    fn recall_returns_context_on_success() {
        let t = FakeTransport::queue(vec![(
            200,
            r#"{"context":"relevant history","memory_count":2,"code":0,"message":""}"#,
        )]);
        let result =
            recall_for_ticket_with_transport(&t, "gah", "/tmp", "#362", "checkpoint resume");
        assert_eq!(result, Some("relevant history".to_string()));
        assert_eq!(t.calls.borrow().len(), 1);
        assert!(t.calls.borrow()[0].0.ends_with("/recall"));
    }

    #[test]
    fn recall_returns_none_on_transport_failure_without_erroring() {
        let t = FakeTransport {
            responses: RefCell::new(vec![Err("connection refused".to_string())].into()),
            calls: RefCell::new(Vec::new()),
        };
        let result =
            recall_for_ticket_with_transport(&t, "gah", "/tmp", "#362", "checkpoint resume");
        assert_eq!(result, None);
    }

    #[test]
    fn recall_returns_none_on_non_200_status() {
        let t = FakeTransport::queue(vec![(401, r#"{"error":"unauthorized"}"#)]);
        let result =
            recall_for_ticket_with_transport(&t, "gah", "/tmp", "#362", "checkpoint resume");
        assert_eq!(result, None);
    }

    #[test]
    fn recall_returns_none_on_degraded_code() {
        let t = FakeTransport::queue(vec![(
            200,
            r#"{"context":"","memory_count":0,"code":1,"message":"degraded"}"#,
        )]);
        let result =
            recall_for_ticket_with_transport(&t, "gah", "/tmp", "#362", "checkpoint resume");
        assert_eq!(result, None);
    }

    #[test]
    fn recall_returns_none_on_empty_context() {
        let t = FakeTransport::queue(vec![(
            200,
            r#"{"context":"   ","memory_count":0,"code":0,"message":""}"#,
        )]);
        let result =
            recall_for_ticket_with_transport(&t, "gah", "/tmp", "#362", "checkpoint resume");
        assert_eq!(result, None);
    }
}
