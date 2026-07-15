use crate::ledger::LedgerEntry;
use crate::worktree;
use std::path::Path;

pub(in crate::dispatch) fn apply_diff_stats(
    ledger: &mut LedgerEntry,
    wt: &Path,
    target_branch: &str,
) {
    if let Ok(stats) = worktree::diff_stats(wt, target_branch) {
        ledger.files_changed = Some(stats.files_changed);
        ledger.insertions = Some(stats.insertions);
        ledger.deletions = Some(stats.deletions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::test_util::{init_repo, profile};
    use std::fs;
    use std::process::Command;

    #[test]
    fn apply_diff_stats_reports_zero_before_commit_but_correct_after() {
        // Other tests temporarily replace PATH with fake backend binaries.
        // Keep every git invocation in this test on the real executable.
        let _exec_guard = crate::test_support::ExecGuard::new();
        // Regression: diff_stats compares origin/<target> against HEAD, so
        // calling apply_diff_stats while real changes are still uncommitted
        // working-tree modifications (HEAD hasn't moved) always reports
        // "0 file(s) changed, +0, -0" -- this is exactly the bug that put
        // that false summary into real MR bodies. dispatch.rs's real call
        // sites now run this after the commit; this test pins why order
        // matters by exercising both states directly.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        let initial_sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // Fake an "origin/main" ref without a real remote, matching how
        // diff_stats/changed_files/has_changes all resolve their comparison
        // point in real dispatch runs.
        Command::new("git")
            .args(["update-ref", "refs/remotes/origin/main", &initial_sha])
            .current_dir(repo)
            .output()
            .unwrap();

        fs::write(repo.join("new_file.txt"), "line one\nline two\n").unwrap();

        let mut prof = profile(repo);
        prof.local_path = repo.display().to_string();
        let mut ledger = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);

        // Before commit: real change exists in the working tree, but HEAD
        // hasn't moved, so the origin/main...HEAD comparison sees nothing.
        apply_diff_stats(&mut ledger, repo, "main");
        assert_eq!(ledger.files_changed, Some(0));

        Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(repo)
            .output()
            .unwrap();

        // After commit: HEAD has moved, so the comparison now sees the
        // real change -- this is what dispatch.rs's real call sites rely on.
        apply_diff_stats(&mut ledger, repo, "main");
        assert_eq!(ledger.files_changed, Some(1));
        assert_eq!(ledger.insertions, Some(2));
        assert_eq!(ledger.deletions, Some(0));
    }
}
