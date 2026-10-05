#!/bin/bash
cat << 'INNER' > claude_diff.patch
--- src/runner/backends/claude.rs
+++ src/runner/backends/claude.rs
@@ -75,25 +75,34 @@
         "launching claude; is it installed and on PATH?",
     )?;
 
-    let output_text = fs::read_to_string(&log_path).unwrap_or_default();
-    if output_text.contains("denies Edit")
-        || output_text.contains("rejected by permission")
-        || output_text.contains("Tool execution was blocked")
-        || output_text.contains("requires approval")
-    {
-        anyhow::bail!("Claude writes were refused. This is a configuration error: ensure claude_args includes --permission-mode acceptEdits and sufficient --allowedTools.");
-    }
-
+    let mut exit_code = exit_code;
     // Locate the transcript for the pinned session id so per-attempt usage
     // parsing can consume it.
     let transcript_path = home
         .as_ref()
         .and_then(|h| find_claude_transcript(h, worktree, &session_id))
         .map(|p| p.to_string_lossy().into_owned());
-    let final_summary = transcript_path
+    let transcript_text = transcript_path
         .as_deref()
-        .and_then(|path| fs::read_to_string(path).ok())
-        .and_then(|text| output::extract_claude_transcript_summary(&text));
+        .and_then(|path| fs::read_to_string(path).ok());
+    let final_summary = transcript_text
+        .as_deref()
+        .and_then(|text| output::extract_claude_transcript_summary(text));
+
+    let output_text = fs::read_to_string(&log_path).unwrap_or_default();
+    let is_refusal = output_text.contains("denies Edit")
+        || output_text.contains("rejected by permission")
+        || output_text.contains("Tool execution was blocked")
+        || output_text.contains("requires approval")
+        || transcript_text.as_deref().is_some_and(|t| t.contains("denies Edit") || t.contains("rejected by permission") || t.contains("Tool execution was blocked") || t.contains("requires approval"));
+
+    if is_refusal {
+        let msg = "\nGAH: Claude writes were refused. This is a configuration error: ensure claude_args includes --permission-mode acceptEdits and sufficient --allowedTools.\n";
+        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path) {
+            use std::io::Write;
+            let _ = f.write_all(msg.as_bytes());
+        }
+        if exit_code == 0 { exit_code = 1; }
+    }
 
     Ok(RunResult {
         exit_code,
INNER
patch -p0 < claude_diff.patch
