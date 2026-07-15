//! Run-scoped usage artifacts for review backends.
//!
//! Worker adapters already capture these sources. Reviews use a specialized
//! supervisor, so they need the same before/after snapshots without falling
//! back to a racy process-wide "latest session" lookup.

use crate::runner::backends::{agy, opencode, vibe};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug)]
pub(crate) enum ReviewUsageCapture {
    Claude {
        session_id: String,
        home: Option<PathBuf>,
    },
    Codex,
    Agy {
        log_path: Option<PathBuf>,
        pre_offset: u64,
    },
    Vibe {
        started_at: SystemTime,
        sessions_before: HashSet<PathBuf>,
    },
    OpenCode {
        started_at: SystemTime,
    },
    None,
}

#[derive(Debug, Default)]
pub(crate) struct ReviewUsageArtifacts {
    pub(crate) artifact_path: Option<String>,
    pub(crate) agy_cli_log_delta: Option<String>,
    /// Authoritative assistant text extracted from a structured backend
    /// stream. `None` means the review runner should keep captured stdout.
    pub(crate) review_text: Option<String>,
}

impl ReviewUsageCapture {
    pub(crate) fn begin(
        backend: &str,
        executable: &Path,
        worktree: &Path,
        env_vars: &[(String, String)],
    ) -> Self {
        match backend {
            "claude" => Self::Claude {
                session_id: uuid::Uuid::new_v4().to_string(),
                home: env_path(env_vars, "HOME").or_else(|| env::var_os("HOME").map(PathBuf::from)),
            },
            "codex" => Self::Codex,
            "agy" | "agy-main" | "agy-second" => {
                let version = agy::detect_agy_version(executable, worktree, env_vars);
                let log_path = agy::agy_cli_log_path(env_vars, executable, version.as_deref());
                let pre_offset = log_path
                    .as_ref()
                    .and_then(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
                    .unwrap_or(0);
                Self::Agy {
                    log_path,
                    pre_offset,
                }
            }
            "vibe" => Self::Vibe {
                started_at: SystemTime::now(),
                sessions_before: vibe::snapshot_vibe_session_metadata_paths(env_vars),
            },
            "opencode" => Self::OpenCode {
                started_at: SystemTime::now(),
            },
            _ => Self::None,
        }
    }

    pub(crate) fn claude_session_id(&self) -> Option<&str> {
        match self {
            Self::Claude { session_id, .. } => Some(session_id),
            _ => None,
        }
    }

    pub(crate) fn finish(
        self,
        worktree: &Path,
        session_dir: &Path,
        raw_stdout: &str,
        env_vars: &[(String, String)],
    ) -> ReviewUsageArtifacts {
        match self {
            Self::Claude { session_id, home } => {
                let artifact_path = home
                    .as_deref()
                    .and_then(|home| {
                        crate::claude_monitor::find_claude_transcript(home, worktree, &session_id)
                    })
                    .map(|path| path.to_string_lossy().into_owned());
                ReviewUsageArtifacts {
                    review_text: artifact_path
                        .as_deref()
                        .and_then(|path| fs::read_to_string(path).ok())
                        .and_then(|text| {
                            crate::runner::output::extract_claude_transcript_summary(&text)
                        }),
                    artifact_path,
                    ..ReviewUsageArtifacts::default()
                }
            }
            Self::Codex => {
                let artifact_path = find_codex_transcript(env_vars, raw_stdout)
                    .map(|path| path.to_string_lossy().into_owned());
                ReviewUsageArtifacts {
                    review_text: crate::runner::output::extract_codex_jsonl_summary(raw_stdout),
                    artifact_path,
                    ..ReviewUsageArtifacts::default()
                }
            }
            Self::Agy {
                log_path,
                pre_offset,
            } => ReviewUsageArtifacts {
                agy_cli_log_delta: agy::log_delta(&log_path, pre_offset),
                ..ReviewUsageArtifacts::default()
            },
            Self::Vibe {
                started_at,
                sessions_before,
            } => ReviewUsageArtifacts {
                artifact_path: vibe::find_vibe_session_metadata(
                    env_vars,
                    worktree,
                    started_at,
                    &sessions_before,
                ),
                ..ReviewUsageArtifacts::default()
            },
            Self::OpenCode { started_at } => {
                let snapshot = opencode::snapshot_opencode_session(
                    env_vars,
                    worktree,
                    started_at,
                    session_dir,
                );
                ReviewUsageArtifacts {
                    artifact_path: snapshot.as_ref().map(|snapshot| snapshot.path.clone()),
                    review_text: snapshot.and_then(|snapshot| snapshot.final_summary),
                    ..ReviewUsageArtifacts::default()
                }
            }
            Self::None => ReviewUsageArtifacts::default(),
        }
    }
}

fn env_path(env_vars: &[(String, String)], name: &str) -> Option<PathBuf> {
    env_vars
        .iter()
        .find_map(|(key, value)| (key == name).then(|| PathBuf::from(value)))
}

pub(crate) fn find_codex_transcript(
    env_vars: &[(String, String)],
    raw_stdout: &str,
) -> Option<PathBuf> {
    let thread_id = raw_stdout.lines().find_map(|line| {
        let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
        (value.get("type").and_then(serde_json::Value::as_str) == Some("thread.started"))
            .then(|| value.get("thread_id").and_then(serde_json::Value::as_str))
            .flatten()
            .map(str::to_string)
    })?;
    let home = env_path(env_vars, "CODEX_HOME")
        .or_else(|| env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| env_path(env_vars, "HOME").map(|home| home.join(".codex")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    let sessions = home.join("sessions");
    // The transcript is normally visible immediately after `codex exec`
    // exits, but tolerate a short filesystem-flush delay.
    for _ in 0..10 {
        if let Some(path) = find_file_named_with(&sessions, &thread_id) {
            return Some(path);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn find_file_named_with(root: &Path, needle: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_named_with(&path, needle) {
                return Some(found);
            }
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains(needle))
        {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_review_capture_binds_json_stream_to_exact_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions/2026/07/15");
        fs::create_dir_all(&sessions).unwrap();
        let thread_id = "019f-review-thread";
        let transcript = sessions.join(format!("rollout-{thread_id}.jsonl"));
        fs::write(
            &transcript,
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4-mini\"}}\n",
        )
        .unwrap();
        let review_json = r#"{"verdict":"APPROVE","confidence":"high"}"#;
        let stdout = format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{thread_id}\"}}\n{{\"type\":\"turn.started\"}}\n{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":{}}}}}\n{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":5}}}}\n",
            serde_json::to_string(review_json).unwrap()
        );
        let env = vec![("CODEX_HOME".to_string(), temp.path().display().to_string())];

        let artifacts = ReviewUsageCapture::Codex.finish(temp.path(), temp.path(), &stdout, &env);

        assert_eq!(artifacts.artifact_path.as_deref(), transcript.to_str());
        assert_eq!(artifacts.review_text.as_deref(), Some(review_json));
    }
}
