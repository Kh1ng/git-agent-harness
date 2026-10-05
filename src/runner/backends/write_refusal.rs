//! Issue #1367: recognise an implementation run whose writes the backend CLI
//! refused (read-only sandbox, permission mode, tool allow-list). Retrying
//! such a run cannot help, so the adapter leaves a GAH-owned marker line in
//! the backend log and the dispatch loop stops on it.
//!
//! Detection reads only the structured records each CLI emits about its own
//! tool calls. Free text (the task prompt, command output, file contents the
//! agent read, the agent's prose) routinely quotes refusal wording and is
//! never consulted.

use serde_json::Value;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::runner::process::worktree_progress_snapshot;

/// Prefix of the log line the adapters append; the rest of the line names
/// the setting to fix.
pub(crate) const WRITE_REFUSED_MARKER: &str = "GAH: backend writes refused (configuration error): ";

/// Claude Code tools that modify the worktree.
const CLAUDE_WRITE_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "Bash"];
const CLAUDE_PERMISSION_DENIAL_PREFIX: &str = "Claude requested permissions to ";
const CODEX_READ_ONLY_SANDBOX: &str = "writing is blocked by read-only sandbox";

/// Worktree content plus HEAD, so both uncommitted edits and commits made by
/// the backend count as changes. `None` when the worktree cannot be inspected.
pub(crate) fn worktree_state(worktree: &Path) -> Option<Vec<u8>> {
    let mut state = worktree_progress_snapshot(worktree)?;
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree)
        .output()
        .ok()?;
    if head.status.success() {
        state.extend_from_slice(&head.stdout);
    }
    Some(state)
}

/// A run that changed the worktree was able to write, whatever else was
/// denied along the way. An uninspectable worktree is reported as unchanged.
pub(crate) fn worktree_changed_since(before: Option<&[u8]>, worktree: &Path) -> bool {
    match (before, worktree_state(worktree)) {
        (Some(before), Some(after)) => before != after.as_slice(),
        _ => false,
    }
}

/// Names of the write tools Claude Code denied for lack of permission, read
/// from the session transcript: an errored `tool_result` carrying the CLI's
/// permission-denial text, joined to the `tool_use` that requested it.
pub(crate) fn claude_refused_write_tools(transcript: &str) -> Vec<String> {
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut refused: Vec<String> = Vec::new();
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for block in content {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) {
                        tool_names.insert(id.to_string(), name.to_string());
                    }
                }
            }
            Some("user") => {
                for block in content {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result")
                        || block.get("is_error").and_then(Value::as_bool) != Some(true)
                        || !tool_result_text(block).starts_with(CLAUDE_PERMISSION_DENIAL_PREFIX)
                    {
                        continue;
                    }
                    let Some(name) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| tool_names.get(id))
                    else {
                        continue;
                    };
                    if CLAUDE_WRITE_TOOLS.contains(&name.as_str()) && !refused.contains(name) {
                        refused.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    refused
}

fn tool_result_text(block: &Value) -> &str {
    match block.get("content") {
        Some(Value::String(text)) => text,
        Some(Value::Array(parts)) => parts
            .iter()
            .find_map(|part| part.get("text").and_then(Value::as_str))
            .unwrap_or(""),
        _ => "",
    }
}

/// Did `codex exec --json` report that the sandbox rejected its writes?
/// Either every patch it attempted failed, or Codex raised its own
/// read-only-sandbox error. Command output and agent messages are ignored.
pub(crate) fn codex_refused_writes(log_text: &str) -> bool {
    let mut failed_patches = 0;
    let mut applied_patches = 0;
    let mut sandbox_error = false;
    for line in log_text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("error") => {
                sandbox_error |= event
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| message.contains(CODEX_READ_ONLY_SANDBOX));
            }
            Some("item.completed") => {
                let item_str = |key: &str| {
                    event
                        .pointer(&format!("/item/{key}"))
                        .and_then(Value::as_str)
                };
                match item_str("type") {
                    Some("file_change") => match item_str("status") {
                        Some("failed") => failed_patches += 1,
                        Some("completed") => applied_patches += 1,
                        _ => {}
                    },
                    Some("error") => {
                        sandbox_error |= item_str("message")
                            .is_some_and(|message| message.contains(CODEX_READ_ONLY_SANDBOX));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    sandbox_error || (failed_patches > 0 && applied_patches == 0)
}

/// Append the marker line to the backend log and return the exit code the
/// attempt should report: a refused run never counts as a success.
pub(crate) fn report(log_path: &Path, exit_code: i32, detail: &str) -> i32 {
    let line = format!("\n{WRITE_REFUSED_MARKER}{detail}\n");
    let appended = OpenOptions::new()
        .append(true)
        .open(log_path)
        .and_then(|mut log| log.write_all(line.as_bytes()));
    if let Err(err) = appended {
        eprintln!(
            "{WRITE_REFUSED_MARKER}{detail} (could not record this in {}: {err})",
            log_path.display()
        );
    }
    if exit_code == 0 {
        1
    } else {
        exit_code
    }
}

/// The setting-naming detail of a marker line written by [`report`]. Only a
/// line that starts with the marker counts, so a log that merely quotes it
/// (inside JSON, a diff, or prose) does not match.
pub(crate) fn refusal_detail(log_text: &str) -> Option<&str> {
    log_text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(WRITE_REFUSED_MARKER))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;

    fn claude_transcript(tool: &str, is_error: bool, result: &str) -> String {
        let prompt = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "Fix #1367: the runner denies Edit and requires approval. Claude requested permissions to use Edit."}
        });
        let call = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": tool, "input": {}}
            ]}
        });
        let outcome = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "is_error": is_error, "content": result}
            ]}
        });
        format!("{prompt}\n{call}\n{outcome}\n")
    }

    #[test]
    fn claude_denied_write_tool_is_reported_by_name() {
        let transcript = claude_transcript(
            "Edit",
            true,
            "Claude requested permissions to write to /repo/src/lib.rs, but you haven't granted it yet.",
        );
        assert_eq!(claude_refused_write_tools(&transcript), vec!["Edit"]);
    }

    #[test]
    fn claude_refusal_wording_in_prompt_or_tool_output_is_not_a_refusal() {
        // The prompt quotes refusal wording, and a successful Read returns a
        // file that contains the CLI's denial sentence.
        let transcript = claude_transcript(
            "Read",
            false,
            "Claude requested permissions to use Edit, but you haven't granted it yet. Tool execution was blocked; rejected by permission.",
        );
        assert!(claude_refused_write_tools(&transcript).is_empty());
    }

    #[test]
    fn claude_denied_read_only_tool_and_ordinary_tool_errors_are_not_write_refusals() {
        let denied_fetch = claude_transcript(
            "WebFetch",
            true,
            "Claude requested permissions to use WebFetch, but you haven't granted it yet.",
        );
        assert!(claude_refused_write_tools(&denied_fetch).is_empty());
        let failed_edit = claude_transcript("Edit", true, "String to replace not found in file.");
        assert!(claude_refused_write_tools(&failed_edit).is_empty());
    }

    #[test]
    fn codex_failed_patches_without_any_applied_patch_are_a_refusal() {
        let log = concat!(
            r#"{"type":"thread.started","thread_id":"t"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"i1","type":"file_change","changes":[{"path":"src/lib.rs","kind":"update"}],"status":"failed"}}"#,
            "\n",
        );
        assert!(codex_refused_writes(log));

        let recovered = format!(
            "{log}{}\n",
            r#"{"type":"item.completed","item":{"id":"i2","type":"file_change","changes":[],"status":"completed"}}"#
        );
        assert!(!codex_refused_writes(&recovered));
    }

    #[test]
    fn codex_sandbox_error_event_is_a_refusal() {
        let log = r#"{"type":"item.completed","item":{"id":"i1","type":"error","message":"patch rejected: writing is blocked by read-only sandbox; rejected by user approval settings"}}"#;
        assert!(codex_refused_writes(log));
    }

    #[test]
    fn codex_refusal_wording_in_command_output_or_agent_message_is_not_a_refusal() {
        // A run that reads this very source file, or the issue text, echoes
        // the sandbox phrase through command output and its own summary.
        let log = concat!(
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"cat src/runner/backends/write_refusal.rs","aggregated_output":"writing is blocked by read-only sandbox","exit_code":0,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Issue #1367 says writing is blocked by read-only sandbox."}}"#,
            "\n",
            "writing is blocked by read-only sandbox\n",
        );
        assert!(!codex_refused_writes(log));
    }

    #[test]
    fn report_appends_marker_and_never_reports_success() {
        let f = fixture();
        let log_path = f.session_dir.join("backend-output.log");
        fs::write(&log_path, "backend output").unwrap();

        assert_eq!(report(&log_path, 0, "fix profile codex_args"), 1);
        assert_eq!(report(&log_path, 7, "fix profile codex_args"), 7);

        let log = fs::read_to_string(&log_path).unwrap();
        assert!(log.starts_with("backend output"));
        assert_eq!(refusal_detail(&log), Some("fix profile codex_args"));
    }

    #[test]
    fn report_still_fails_the_attempt_when_the_log_is_unwritable() {
        let f = fixture();
        let missing = f.session_dir.join("absent").join("backend-output.log");
        assert_eq!(report(&missing, 0, "detail"), 1);
    }

    #[test]
    fn refusal_detail_ignores_quoted_markers() {
        let quoted = format!(
            "{{\"aggregated_output\":\"{WRITE_REFUSED_MARKER}x\"}}\n+ {WRITE_REFUSED_MARKER}y\nThis is a configuration error\n"
        );
        assert_eq!(refusal_detail(&quoted), None);
    }

    #[test]
    fn worktree_changes_include_edits_and_commits() {
        let f = fixture();
        assert!(worktree_state(&f.worktree).is_none());
        assert!(!worktree_changed_since(None, &f.worktree));

        initialize_git_worktree(&f.worktree);
        let before = worktree_state(&f.worktree);
        assert!(!worktree_changed_since(before.as_deref(), &f.worktree));

        fs::write(f.worktree.join("progress.txt"), "edited\n").unwrap();
        assert!(worktree_changed_since(before.as_deref(), &f.worktree));

        let status = Command::new("git")
            .args(["commit", "-qam", "agent commit"])
            .current_dir(&f.worktree)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(worktree_changed_since(before.as_deref(), &f.worktree));
    }
}
