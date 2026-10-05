#!/bin/bash
cat << 'INNER' > stall_diff.patch
--- src/dispatch/workflows/improve/stall.rs
+++ src/dispatch/workflows/improve/stall.rs
@@ -32,8 +32,11 @@
         stalled && log_text.contains("stalled during validation with checkpointed changes");
     let cleanup_failed = exit_code == crate::runner::process::PROCESS_CLEANUP_FAILED_EXIT_CODE
         || log_text.contains("GAH: harness process cleanup failed:");
+    let is_config_error = log_text.contains("This is a configuration error");
     let failure_class = if cleanup_failed {
         crate::ledger::FailureClass::HarnessError
+    } else if is_config_error {
+        crate::ledger::FailureClass::EnvironmentError
     } else if stalled_before_changes {
         crate::ledger::FailureClass::AgentNoProgress
     } else if stalled {
INNER
patch -p0 < stall_diff.patch
