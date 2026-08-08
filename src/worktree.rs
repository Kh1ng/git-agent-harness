use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus, Output, Stdio};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(300);
const GIT_NETWORK_ATTEMPTS: u8 = 2;
#[cfg(not(test))]
const GIT_NETWORK_RETRY_BACKOFF: Duration = Duration::from_secs(10);

/// Return true only for transport failures that are normally transient.
///
/// Authentication, authorization, non-fast-forward, and ordinary git errors
/// deliberately do not match: retrying them would hide a real configuration
/// or repository problem behind a pointless delay.
pub fn is_transient_network_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "connection timed out",
        "connection reset",
        "could not resolve host",
        "early eof",
        "tls handshake timeout",
        "ssh_exchange_identification",
    ]
    .iter()
    .any(|signature| text.contains(signature))
}

fn git_network_retry_backoff() -> Duration {
    #[cfg(test)]
    {
        Duration::ZERO
    }
    #[cfg(not(test))]
    {
        GIT_NETWORK_RETRY_BACKOFF
    }
}

fn retry_transient_git_network<T>(
    operation: &str,
    mut attempt: impl FnMut() -> Result<T>,
) -> Result<T> {
    for number in 1..=GIT_NETWORK_ATTEMPTS {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(err)
                if number < GIT_NETWORK_ATTEMPTS
                    && is_transient_network_error(&format!("{err:#}")) =>
            {
                eprintln!(
                    "transient git network failure during {operation}; retrying {}/{} after {}s: {:#}",
                    number + 1,
                    GIT_NETWORK_ATTEMPTS,
                    git_network_retry_backoff().as_secs(),
                    err
                );
                thread::sleep(git_network_retry_backoff());
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("bounded git retry loop always returns")
}

fn wait_with_timeout(child: Child, context: &str) -> Result<Output> {
    wait_with_timeout_for(child, context, GIT_TIMEOUT)
}

fn drain_pipe<R>(mut pipe: R) -> JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn collect_pipe(
    reader: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    context: &str,
    stream: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("{context} {stream} reader panicked"))?
        .with_context(|| format!("reading {context} {stream}"))
}

fn wait_with_timeout_for(mut child: Child, context: &str, timeout: Duration) -> Result<Output> {
    // Read both pipes while the child runs. Waiting for exit before draining
    // deadlocks as soon as git emits more than the OS pipe capacity: git
    // blocks in write(2), cannot exit, and the parent waits until timeout.
    let stdout = child
        .stdout
        .take()
        .map(|pipe: ChildStdout| drain_pipe(pipe));
    let stderr = child
        .stderr
        .take()
        .map(|pipe: ChildStderr| drain_pipe(pipe));
    let started = Instant::now();
    let status: ExitStatus = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            // Killing the child closes its pipe ends, allowing the drain
            // threads to terminate before this function returns.
            let _ = collect_pipe(stdout, context, "stdout");
            let _ = collect_pipe(stderr, context, "stderr");
            anyhow::bail!("{context} timed out after {}s", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    };

    Ok(Output {
        status,
        stdout: collect_pipe(stdout, context, "stdout")?,
        stderr: collect_pipe(stderr, context, "stderr")?,
    })
}

fn fetch_origin(repo: &Path) -> Result<()> {
    // `.git` is a file in a linked worktree, not a directory. Resolve the
    // shared common Git directory so every checkout of the same repository
    // serializes fetches on one lock and linked-worktree profiles work too.
    let git_common_dir = PathBuf::from(git(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        repo,
    )?);
    let lock_path = git_common_dir.join("gah-fetch.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening fetch lock {}", lock_path.display()))?;
    lock.lock_exclusive().context("locking shared git fetch")?;
    let result =
        retry_transient_git_network("fetch", || git(&["fetch", "-q", "origin", "--prune"], repo));
    FileExt::unlock(&lock).ok();
    result.map(|_| ())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DiffStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Content-sensitive state used to prove that one backend attempt changed its
/// own starting worktree. This is intentionally different from `has_changes`,
/// which answers whether the branch differs from the target branch and is
/// therefore already true before every FixMr attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStateSnapshot(Vec<u8>);

pub fn state_snapshot(worktree: &Path) -> Result<WorktreeStateSnapshot> {
    let head = git_raw(&["rev-parse", "HEAD"], worktree)?;
    let diff = git_raw(
        &["diff", "--no-ext-diff", "--binary", "HEAD", "--"],
        worktree,
    )?;
    let staged = git_raw(
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--binary",
            "HEAD",
            "--",
        ],
        worktree,
    )?;
    let status = git_raw(
        &["status", "--porcelain", "--untracked-files=all"],
        worktree,
    )?;
    for (name, output) in [
        ("rev-parse HEAD", &head),
        ("diff HEAD", &diff),
        ("diff --cached HEAD", &staged),
        ("status", &status),
    ] {
        if !output.status.success() {
            anyhow::bail!(
                "git {name}: {}",
                crate::redact::redact(&String::from_utf8_lossy(&output.stderr)).trim()
            );
        }
    }

    let mut bytes = head.stdout;
    bytes.extend_from_slice(&diff.stdout);
    bytes.extend_from_slice(&staged.stdout);
    bytes.extend_from_slice(&status.stdout);
    Ok(WorktreeStateSnapshot(bytes))
}

pub fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = git_raw(args, cwd)?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            crate::redact::redact(&String::from_utf8_lossy(&out.stderr)).trim()
        );
    }
    Ok(crate::redact::redact(&String::from_utf8_lossy(&out.stdout))
        .trim()
        .to_string())
}

/// Run git and return raw Output. Does NOT error on non-zero exit.
pub fn git_raw(args: &[&str], cwd: &Path) -> Result<std::process::Output> {
    let child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("git {}", args.join(" ")))?;
    wait_with_timeout(child, &format!("git {}", args.join(" ")))
}

pub fn create(
    repo: &Path,
    target_branch: &str,
    new_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    fetch_origin(repo)?;

    let origin_ref = format!("origin/{}", target_branch);
    let worktree_path = worktree_base.join(new_branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap_or(worktree_base))?;

    git(
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            new_branch,
            worktree_path.to_str().unwrap(),
            &origin_ref,
        ],
        repo,
    )
    .with_context(|| format!("creating worktree from {}", origin_ref))?;

    Ok(worktree_path)
}

pub fn create_existing(
    repo: &Path,
    existing_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    fetch_origin(repo)?;

    let origin_ref = format!("origin/{}", existing_branch);
    let worktree_path = worktree_base.join(existing_branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap_or(worktree_base))?;

    // Never force-remove an existing path here. Its pathname is not proof
    // that GAH owns it, and even a GAH-created worktree can still belong to a
    // live worker or contain uncommitted recovery data. Lifecycle pruning is
    // the only subsystem allowed to remove retained worktrees.
    if worktree_path.exists() {
        anyhow::bail!(
            "refusing to replace existing worktree path {}; prune or remove it explicitly after verifying it is inactive",
            worktree_path.display()
        );
    }

    // `-B existing_branch` creates/resets a real local branch tracking
    // origin_ref instead of leaving the worktree in detached HEAD.
    // Without this, `git push origin <existing_branch>` from the worktree
    // silently exits 0 while pushing nothing -- there's no local ref by
    // that name to serve as the push source, since detached HEAD isn't
    // one. Confirmed by reproduction: a commit made on a detached-HEAD
    // worktree checkout of `origin/<branch>` never reached the remote
    // branch even though `git push` reported success.
    git(
        &[
            "worktree",
            "add",
            "-q",
            "-B",
            existing_branch,
            worktree_path.to_str().unwrap(),
            &origin_ref,
        ],
        repo,
    )
    .with_context(|| format!("creating worktree from existing branch {}", origin_ref))?;

    Ok(worktree_path)
}

/// Result of bringing an existing repair branch up to date with its target.
/// A conflict is a repairable repository state, not a generic preflight
/// failure: the caller may hand the live merge to a bounded repair backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRefreshOutcome {
    AlreadyCurrent,
    Merged,
    Conflicted {
        target_ref: String,
        files: Vec<String>,
        details: String,
    },
}

/// Bring an existing repair branch up to date with the exact target ref
/// fetched immediately before its worktree was created.
///
/// Long-lived draft PRs routinely outlive fixes merged to the target branch.
/// Validating such a branch before incorporating the target can therefore
/// fail on already-fixed infrastructure or flaky tests and repeatedly block
/// the repair before an agent launches. A local merge preserves the PR's
/// history and lets the normal successful publish push the refresh together
/// with the repair. The caller snapshots attempt progress after this step, so
/// this maintenance merge cannot masquerade as agent work.
pub fn refresh_existing_branch_from_target(
    worktree: &Path,
    target_branch: &str,
) -> Result<TargetRefreshOutcome> {
    let target_ref = format!("origin/{target_branch}");
    let ancestor = git_raw(
        &["merge-base", "--is-ancestor", &target_ref, "HEAD"],
        worktree,
    )?;
    if ancestor.status.success() {
        return Ok(TargetRefreshOutcome::AlreadyCurrent);
    }
    if ancestor.status.code() != Some(1) {
        anyhow::bail!(
            "checking whether repair branch contains {target_ref}: {}",
            crate::redact::redact(&String::from_utf8_lossy(&ancestor.stderr)).trim()
        );
    }

    let merge = git_raw(
        &[
            "-c",
            "user.name=GAH",
            "-c",
            "user.email=gah@localhost",
            "merge",
            "--no-edit",
            "--no-stat",
            &target_ref,
        ],
        worktree,
    )?;
    if merge.status.success() {
        return Ok(TargetRefreshOutcome::Merged);
    }

    let merge_error = bounded_git_failure_details(&merge.stdout, &merge.stderr);
    let files = unmerged_files(worktree)?;
    if files.is_empty() {
        let abort = git_raw(&["merge", "--abort"], worktree)?;
        if !abort.status.success() {
            anyhow::bail!(
                "refreshing repair branch from {target_ref} failed without a resolvable conflict: {merge_error}; additionally failed to abort merge: {}",
                crate::redact::redact(&String::from_utf8_lossy(&abort.stderr)).trim()
            );
        }
        anyhow::bail!(
            "refreshing repair branch from {target_ref} failed without unmerged entries: {merge_error}"
        );
    }
    Ok(TargetRefreshOutcome::Conflicted {
        target_ref,
        files,
        details: merge_error,
    })
}

/// Return the unique paths whose index still contains unmerged stages.
pub fn unmerged_files(worktree: &Path) -> Result<Vec<String>> {
    let output = git_raw(&["diff", "--name-only", "--diff-filter=U", "-z"], worktree)?;
    if !output.status.success() {
        anyhow::bail!(
            "listing unmerged files: {}",
            crate::redact::redact(&String::from_utf8_lossy(&output.stderr)).trim()
        );
    }
    let mut files: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// A backend can stage a file while accidentally retaining conflict markers.
/// Check only paths that Git originally reported as conflicted, avoiding a
/// repository-wide heuristic over legitimate fixture text.
pub fn files_with_conflict_markers(worktree: &Path, files: &[String]) -> Result<Vec<String>> {
    let mut marked = Vec::new();
    for file in files {
        let path = worktree.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        if text.lines().any(|line| {
            line.starts_with("<<<<<<< ")
                || line.starts_with("||||||| ")
                || line == "======="
                || line.starts_with(">>>>>>> ")
        }) {
            marked.push(file.clone());
        }
    }
    Ok(marked)
}

pub fn merge_in_progress(worktree: &Path) -> Result<bool> {
    let output = git_raw(&["rev-parse", "-q", "--verify", "MERGE_HEAD"], worktree)?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    anyhow::bail!(
        "checking MERGE_HEAD: {}",
        crate::redact::redact(&String::from_utf8_lossy(&output.stderr)).trim()
    )
}

pub fn target_is_ancestor(worktree: &Path, target_branch: &str) -> Result<bool> {
    let target_ref = format!("origin/{target_branch}");
    let output = git_raw(
        &["merge-base", "--is-ancestor", &target_ref, "HEAD"],
        worktree,
    )?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    anyhow::bail!(
        "checking target ancestry for {target_ref}: {}",
        crate::redact::redact(&String::from_utf8_lossy(&output.stderr)).trim()
    )
}

/// Preserve a conflicted index as a bounded, local recovery artifact before
/// removing its worktree. Git cannot commit or stash unmerged entries, so the
/// working and staged binary patches plus a manifest are the durable handoff.
pub fn preserve_conflict_recovery(
    worktree: &Path,
    artifact_dir: &Path,
    target_branch: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(artifact_dir)?;
    let files = unmerged_files(worktree)?;
    let head = git(&["rev-parse", "HEAD"], worktree)?;
    let merge_head = git(&["rev-parse", "MERGE_HEAD"], worktree)?;
    let working = git_raw(&["diff", "--binary", "--no-ext-diff"], worktree)?;
    let staged = git_raw(&["diff", "--cached", "--binary", "--no-ext-diff"], worktree)?;
    std::fs::write(artifact_dir.join("working.patch"), working.stdout)?;
    std::fs::write(artifact_dir.join("staged.patch"), staged.stdout)?;
    std::fs::write(
        artifact_dir.join("manifest.txt"),
        format!(
            "target=origin/{target_branch}\nhead={head}\nmerge_head={merge_head}\nunmerged_files={}\n",
            files.join(",")
        ),
    )?;
    Ok(artifact_dir.to_path_buf())
}

fn bounded_git_failure_details(stdout: &[u8], stderr: &[u8]) -> String {
    const MAX_BYTES: usize = 4_000;
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined = match (stderr.trim(), stdout.trim()) {
        ("", "") => "git exited non-zero without diagnostic output".to_string(),
        ("", stdout) => stdout.to_string(),
        (stderr, "") => stderr.to_string(),
        (stderr, stdout) => format!("{stderr}\n{stdout}"),
    };
    let redacted = crate::redact::redact(&combined);
    if redacted.len() <= MAX_BYTES {
        return redacted;
    }
    let mut end = MAX_BYTES;
    while end > 0 && !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[git diagnostic truncated]", &redacted[..end])
}

/// Describes a worktree currently attached to a branch, as reported by
/// `git worktree list`, if one exists.
pub struct BranchWorktreeAttachment {
    /// Absolute path of the attached worktree.
    pub path: PathBuf,
    /// True when the attached worktree has no uncommitted changes.
    pub clean: bool,
}

/// Returns the worktree (if any) currently attached to `branch`. A branch
/// checked out in a worktree cannot be reused by `git worktree add`; doing so
/// fails with "branch is already used by worktree at '<path>'", which would
/// otherwise terminate a recurring `gah loop`. Detect this before dispatch so
/// the controller can defer the work item instead of stalling on a hard git
/// failure.
///
/// Detection deliberately does not infer ownership from path. Any attachment
/// can be live or dirty and must be deferred until lifecycle cleanup proves it
/// safe to remove.
pub fn branch_attachment(repo: &Path, branch: &str) -> Result<Option<BranchWorktreeAttachment>> {
    let out = git_raw(&["worktree", "list", "--porcelain"], repo)?;
    if !out.status.success() {
        // If we can't enumerate worktrees, err on the side of attempting the
        // dispatch -- better a surfaced transient failure than a silent skip.
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut current_path: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(path) = &current_path {
                let branch_ref = format!("refs/heads/{}", branch);
                if b.trim() == branch_ref {
                    let clean = path.exists() && matches!(has_uncommitted_changes(path), Ok(false));
                    return Ok(Some(BranchWorktreeAttachment {
                        path: path.clone(),
                        clean,
                    }));
                }
            }
        }
    }
    Ok(None)
}

pub fn has_changes(worktree: &Path, target_branch: &str) -> Result<bool> {
    if has_uncommitted_changes(worktree)? {
        return Ok(true);
    }
    // ponytail: compare against origin/<target> — @{upstream} fails silently on new untracked branches
    let origin_ref = format!("origin/{}", target_branch);
    let diff = git_raw(&["diff", "HEAD", &origin_ref], worktree)?;
    Ok(!diff.stdout.is_empty())
}

/// Some backends (e.g. vibe) commit their own work during the run instead of
/// leaving a dirty working tree for GAH to stage. `has_changes` can be true
/// purely from those already-committed commits sitting ahead of the target
/// branch -- callers must check this separately before staging, or
/// `ensure_staged` fails loudly on a clean tree ("nothing to commit").
pub fn has_uncommitted_changes(worktree: &Path) -> Result<bool> {
    let status = git_raw(&["status", "--porcelain"], worktree)?;
    Ok(!status.stdout.is_empty())
}

#[allow(dead_code)]
pub fn diff_patch(worktree: &Path, target_branch: &str) -> Result<String> {
    let origin_ref = format!("origin/{}", target_branch);
    Ok(
        String::from_utf8_lossy(&git_raw(&["diff", &origin_ref, "HEAD"], worktree)?.stdout)
            .to_string(),
    )
}

#[allow(dead_code)]
pub fn changed_files(worktree: &Path, target_branch: &str) -> Result<Vec<String>> {
    let origin_ref = format!("origin/{}", target_branch);
    let out = git_raw(&["diff", "--name-only", &origin_ref, "HEAD"], worktree)?;
    let tracked = String::from_utf8_lossy(&out.stdout).to_string();
    let mut files: Vec<String> = tracked
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    let status = git_raw(&["status", "--porcelain"], worktree)?;
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let first = line.as_bytes().first().copied().unwrap_or(b' ');
        let second = line.as_bytes().get(1).copied().unwrap_or(b' ');
        if first != b' ' || second != b' ' {
            files.push(line[3..].trim().to_string());
        }
    }
    Ok(files)
}

pub fn diff_stats(worktree: &Path, target_branch: &str) -> Result<DiffStats> {
    let origin_ref = format!("origin/{}", target_branch);
    let out = git_raw(&["diff", "--numstat", &origin_ref, "HEAD"], worktree)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut stats = DiffStats::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(adds) = parts.next() else { continue };
        let Some(dels) = parts.next() else { continue };
        stats.files_changed += 1;
        stats.insertions += adds.parse::<u32>().unwrap_or(0);
        stats.deletions += dels.parse::<u32>().unwrap_or(0);
    }
    Ok(stats)
}

#[allow(dead_code)]
pub fn commit_and_push(
    worktree: &Path,
    branch: &str,
    push_url: &str,
    repo_id: &str,
    pat: &str,
) -> Result<()> {
    stage_all(worktree)?;
    ensure_staged(worktree)?;
    commit_msg(
        worktree,
        &format!("gah: improve mode changes for {}", repo_id),
    )?;
    push_branch(worktree, branch, push_url, pat)
}

/// Write a temporary GIT_ASKPASS script that outputs the given password.
/// Returns the path to the script. The caller MUST clean up the file.
fn write_askpass(pat: &str) -> Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!("gah-askpass-{}", std::process::id()));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(b"#!/bin/sh\n")?;
    f.write_all(b"echo \"")?;
    f.write_all(pat.as_bytes())?;
    f.write_all(b"\"\n")?;
    // Make executable
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

pub fn stage_all(worktree: &Path) -> Result<()> {
    git(&["add", "-A"], worktree)?;
    Ok(())
}

pub fn ensure_staged(worktree: &Path) -> Result<()> {
    let staged = git_raw(&["diff", "--cached", "--name-only"], worktree)?;
    if staged.stdout.is_empty() {
        anyhow::bail!("nothing to commit after git add -A");
    }
    Ok(())
}

pub fn commit_msg(worktree: &Path, msg: &str) -> Result<()> {
    git(&["commit", "-q", "-m", msg], worktree)?;
    Ok(())
}

/// Make the current changed tree durable before a dispatch discards or
/// replaces it. Backends sometimes commit themselves; otherwise this creates
/// a local WIP commit. The dispatch branch remains the recovery point for a
/// terminal failure, while retry callers may additionally retain `HEAD` on a
/// checkpoint branch before resetting the working branch to its target.
pub fn preserve_wip(worktree: &Path, target_branch: &str, message: &str) -> Result<bool> {
    // Git cannot commit or stash an unmerged index. Conflict-resolution
    // callers preserve a binary recovery artifact before cleanup; returning
    // false here prevents a secondary commit error from hiding the typed
    // conflict outcome.
    if !unmerged_files(worktree)?.is_empty() {
        return Ok(false);
    }
    if !has_changes(worktree, target_branch)? {
        return Ok(false);
    }
    if has_uncommitted_changes(worktree)? {
        stage_all(worktree)?;
        ensure_staged(worktree)?;
        commit_msg(worktree, message)?;
    }
    Ok(true)
}

/// Commit changed work and preserve the resulting HEAD under a dedicated
/// checkpoint branch. The caller decides whether a retry should continue from
/// that HEAD or reset to the target. The checkpoint is local and is removed
/// only after the overall dispatch publishes successfully.
pub fn checkpoint_wip(
    worktree: &Path,
    target_branch: &str,
    checkpoint_branch: &str,
    message: &str,
) -> Result<bool> {
    if !preserve_wip(worktree, target_branch, message)? {
        return Ok(false);
    }
    git(&["branch", "-f", checkpoint_branch, "HEAD"], worktree)?;
    Ok(true)
}

/// Return a retry worktree to the configured target without moving any WIP
/// checkpoint ref created by `checkpoint_wip`.
pub fn reset_to_target(worktree: &Path, target_branch: &str) -> Result<()> {
    let target = format!("origin/{target_branch}");
    git(&["reset", "--hard", &target], worktree)?;
    git(&["clean", "-fd"], worktree)?;
    Ok(())
}

/// Issue #537: a repair retry's clean base is the repair branch's own remote
/// tip, never the profile's default target branch -- resetting a repair to
/// `origin/main` between attempts silently discards the PR being repaired,
/// leaving the next attempt to build fresh work on `main` that then fails to
/// push (non-fast-forward) against the real, diverged remote branch.
///
/// Fetches `origin` and returns the current tip SHA of `origin/<branch>`
/// without moving the worktree. The caller must compare this against the SHA
/// observed when the repair began and refuse to reset (call
/// `reset_to_target`) if it moved -- a concurrent push during the retry loop
/// must fail closed, never be silently discarded by a reset.
pub fn fetch_remote_branch_sha(repo: &Path, worktree: &Path, branch: &str) -> Result<String> {
    fetch_origin(repo)?;
    Ok(git(&["rev-parse", &format!("origin/{branch}")], worktree)?
        .trim()
        .to_string())
}

/// Best-effort removal of local, successful-dispatch-only WIP checkpoint
/// refs. Never use this for terminal failures: those refs are recovery data.
pub fn delete_local_branch(repo: &Path, branch: &str) -> Result<()> {
    git(&["branch", "-D", branch], repo)?;
    Ok(())
}

pub fn push_branch(worktree: &Path, branch: &str, push_url: &str, pat: &str) -> Result<()> {
    push_branch_with_executable(Path::new("git"), worktree, branch, push_url, pat)
}

fn push_branch_with_executable(
    executable: &Path,
    worktree: &Path,
    branch: &str,
    push_url: &str,
    pat: &str,
) -> Result<()> {
    let askpass = write_askpass(pat)?;
    let result = retry_transient_git_network("push", || {
        let child = Command::new(executable)
            .args(["push", "-q", push_url, branch])
            .env("GIT_ASKPASS", &askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(worktree)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("git push")?;
        let out = wait_with_timeout(child, "git push")?;
        if !out.status.success() {
            anyhow::bail!(
                "push failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    });
    let _ = std::fs::remove_file(&askpass);
    result
}

/// Deletes a remote branch (`git push <url> --delete <branch>`), used by
/// `gah prune` to clean up merged/closed PR/MR branches the provider
/// doesn't always auto-delete on its own. `worktree` only needs to be some
/// git working directory to run the command from -- `branch` doesn't need
/// to be checked out anywhere.
pub fn delete_remote_branch(
    worktree: &Path,
    branch: &str,
    push_url: &str,
    pat: &str,
) -> Result<()> {
    delete_remote_branch_with_executable(Path::new("git"), worktree, branch, push_url, pat)
}

fn delete_remote_branch_with_executable(
    executable: &Path,
    worktree: &Path,
    branch: &str,
    push_url: &str,
    pat: &str,
) -> Result<()> {
    let askpass = write_askpass(pat)?;
    let result = retry_transient_git_network("push --delete", || {
        let child = Command::new(executable)
            .args(["push", "-q", push_url, "--delete", branch])
            .env("GIT_ASKPASS", &askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(worktree)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("git push --delete")?;
        let out = wait_with_timeout(child, "git push --delete")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // Already gone (deleted manually, or by a prior prune run) is
            // not a failure worth surfacing.
            if stderr.contains("remote ref does not exist") {
                return Ok(());
            }
            anyhow::bail!("delete failed: {}", stderr.trim());
        }
        Ok(())
    });
    let _ = std::fs::remove_file(&askpass);
    result
}

#[allow(dead_code)]
pub fn commit_and_push_msg(
    worktree: &Path,
    branch: &str,
    push_url: &str,
    msg: &str,
    pat: &str,
) -> Result<()> {
    stage_all(worktree)?;
    ensure_staged(worktree)?;
    commit_msg(worktree, msg)?;
    push_branch(worktree, branch, push_url, pat)
}

pub fn cleanup(worktree: &Path, repo: &Path) {
    let _ = git_raw(
        &["worktree", "remove", "-f", worktree.to_str().unwrap_or("")],
        repo,
    );
    let _ = git_raw(&["worktree", "prune"], repo);
}

#[cfg(test)]
#[path = "worktree/tests.rs"]
mod tests;
