/**
 * Git data caching module. Bounds CLI calls for git/gh/glab operations
 * with an in-memory TTL cache so that UI surfaces render instantly on
 * repeat and never hammer the provider.
 */
import { spawnSync } from 'node:child_process';
import type { GitReviewState } from '@git-agent-harness/contracts';
import { AsyncTtlCache } from './asyncTtlCache.js';

const DEFAULT_GIT_CACHE_TTL_MS = 15_000; // 15 seconds TTL for git data
// Bounds on every git/gh/glab subprocess: a hung or chatty provider call
// must not block a request indefinitely or buffer unbounded output.
const SUBPROCESS_TIMEOUT_MS = 10_000;
const MAX_OUTPUT_BYTES = 5 * 1024 * 1024;

interface GitStatusResult {
  branch: string;
  changes: { status: string; path: string }[];
  cwd: string;
}

interface GitBranchesResult {
  branches: string[];
  current: string;
}

interface GitLogResult {
  commits: { hash: string; short: string; subject: string; author: string; ago: string }[];
}

interface GitCommitResult {
  hash: string;
}

export type GitWorktreeReview = Omit<GitReviewState,
  'ownerNodeId' | 'ownerNodeName' | 'provider' | 'providerLabel' | 'existing'>;

function gitInDir(cwd: string, args: string[]): { ok: boolean; out: string; err: string } {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', timeout: SUBPROCESS_TIMEOUT_MS, maxBuffer: MAX_OUTPUT_BYTES });
  return { ok: result.status === 0, out: result.stdout ?? '', err: result.stderr ?? '' };
}

function gitOutput(cwd: string, args: string[]): string {
  const result = gitInDir(cwd, args);
  if (!result.ok) throw new Error(result.err || result.out || `git ${args[0]} failed`);
  return result.out;
}

function nulList(value: string): string[] {
  return value.split('\0').filter(Boolean);
}

function changedFiles(cwd: string) {
  const staged = new Set(nulList(gitOutput(cwd, ['diff', '--cached', '--name-only', '-z', '--no-renames'])));
  const unstaged = new Set(nulList(gitOutput(cwd, ['diff', '--name-only', '-z', '--no-renames'])));
  const untracked = new Set(nulList(gitOutput(cwd, ['ls-files', '--others', '--exclude-standard', '-z'])));
  const paths = [...new Set([...staged, ...unstaged, ...untracked])].sort();
  return {
    paths,
    staged,
    unstaged,
    untracked,
    files: paths.map(path => ({ path, staged: staged.has(path), unstaged: unstaged.has(path), untracked: untracked.has(path) }))
  };
}

function selectedChanges(cwd: string, files: string[]) {
  const selected = [...new Set(files)];
  if (selected.length === 0 || selected.some(path => !path || /[\0\r\n]/.test(path))) throw new Error('Select at least one valid changed file');
  const changes = changedFiles(cwd);
  if (selected.some(path => !changes.paths.includes(path))) throw new Error('Selected files must be current worktree changes');
  return { selected, changes };
}

function helperSafePath(path: string): boolean {
  const name = path.split('/').at(-1) ?? path;
  return !/^\.env(?:\.|$)/i.test(name)
    && !/(?:auth|credential|secret|token)s?(?:\.|$)/i.test(name)
    && !/\.(?:key|pem|p12|pfx)$/i.test(name)
    && !/^(?:\.npmrc|\.pypirc|\.netrc|id_(?:rsa|dsa|ecdsa|ed25519))$/i.test(name);
}

function sanitizeHelperPatch(patch: string): string {
  return patch.split('\n').map(line => /(?:password|passwd|secret|token|api[_-]?key|authorization|private[_-]?key)["']?\s*[:=]/i.test(line)
    ? `${line[0] === '+' || line[0] === '-' || line[0] === ' ' ? line[0] : ''}[credential line omitted]`
    : line).join('\n');
}

/** Exact working diff for a reviewed file selection. Unrelated dirty files
 * never enter helper-model input. */
export function getSelectedChangesPatch(cwd: string, files: string[]): string {
  const { selected, changes } = selectedChanges(cwd, files);
  if (selected.some(path => !helperSafePath(path))) throw new Error('Credential and environment files cannot be sent to a helper model');
  const tracked = selected.filter(path => !changes.untracked.has(path));
  const chunks = tracked.length > 0 ? [gitOutput(cwd, ['diff', '--no-ext-diff', '--no-color', 'HEAD', '--', ...tracked])] : [];
  for (const path of selected.filter(candidate => changes.untracked.has(candidate))) {
    const diff = gitInDir(cwd, ['diff', '--no-ext-diff', '--no-color', '--no-index', '--', '/dev/null', path]);
    if (diff.ok || diff.out) chunks.push(diff.out);
  }
  return sanitizeHelperPatch(chunks.join('\n'));
}

/** Committed diff for helper input, excluding credential and environment files. */
export function getReviewHelperPatch(cwd: string, requestedBase?: string): string {
  const { ref } = reviewBase(cwd, requestedBase);
  const files = nulList(gitOutput(cwd, ['diff', '--name-only', '-z', '--no-renames', `${ref}...HEAD`])).filter(helperSafePath);
  return files.length === 0 ? '' : sanitizeHelperPatch(gitOutput(cwd, ['diff', '--no-ext-diff', '--no-color', `${ref}...HEAD`, '--', ...files]));
}

function reviewBase(cwd: string, requested?: string): { base: string; ref: string } {
  let base = requested?.trim().replace(/^origin\//, '') ?? '';
  if (base) {
    if (!gitInDir(cwd, ['check-ref-format', '--branch', base]).ok) throw new Error('Invalid base branch');
  } else {
    const symbolic = gitInDir(cwd, ['symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD']);
    base = symbolic.ok ? symbolic.out.trim().replace(/^origin\//, '') : '';
    if (!base) {
      base = gitInDir(cwd, ['rev-parse', '--verify', '--quiet', 'refs/remotes/origin/main']).ok ? 'main'
        : gitInDir(cwd, ['rev-parse', '--verify', '--quiet', 'refs/remotes/origin/master']).ok ? 'master'
          : gitOutput(cwd, ['branch', '--show-current']).trim();
    }
  }
  const remote = `refs/remotes/origin/${base}`;
  const ref = gitInDir(cwd, ['rev-parse', '--verify', '--quiet', remote]).ok ? remote : base;
  if (!gitInDir(cwd, ['rev-parse', '--verify', '--quiet', ref]).ok) throw new Error(`Base branch '${base}' is unavailable locally`);
  return { base, ref };
}

/** Returns the committed and uncommitted state that a user must review before
 * committing or publishing. This function never fetches, stages, or pushes. */
export async function getGitReviewState(cwd: string, requestedBase?: string): Promise<GitWorktreeReview> {
  const branch = gitOutput(cwd, ['branch', '--show-current']).trim();
  if (!branch) throw new Error('A named branch is required');
  const { base, ref } = reviewBase(cwd, requestedBase);
  const upstreamResult = gitInDir(cwd, ['rev-parse', '--abbrev-ref', '--symbolic-full-name', '@{upstream}']);
  const upstream = upstreamResult.ok ? upstreamResult.out.trim() : null;
  let ahead = 0;
  let behind = 0;
  if (upstream) {
    const counts = gitOutput(cwd, ['rev-list', '--left-right', '--count', `${upstream}...HEAD`]).trim().split(/\s+/).map(Number);
    behind = counts[0] || 0;
    ahead = counts[1] || 0;
  }
  const records = gitOutput(cwd, ['log', `${ref}..HEAD`, '--format=%H%x1f%h%x1f%s%x1e'])
    .split('\x1e').map(record => record.trim()).filter(Boolean);
  const commits = records.map(record => {
    const [hash, short, ...subject] = record.split('\x1f');
    return { hash, short, subject: subject.join('\x1f') };
  });
  const changes = changedFiles(cwd);
  return {
    branch,
    base,
    upstream,
    ahead,
    behind,
    files: changes.files,
    commits,
    changedFiles: nulList(gitOutput(cwd, ['diff', '--name-only', '-z', '--no-renames', `${ref}...HEAD`])).sort(),
    patch: gitOutput(cwd, ['diff', '--no-ext-diff', '--no-color', `${ref}...HEAD`])
  };
}

/** Runs a provider mutation with the same timeout/output bounds as git. */
export function cliInDir(bin: string, args: string[], cwd: string): { ok: boolean; out: string } {
  const result = spawnSync(bin, args, { cwd, encoding: 'utf8', timeout: SUBPROCESS_TIMEOUT_MS, maxBuffer: MAX_OUTPUT_BYTES });
  return { ok: result.status === 0, out: result.stdout ?? '' };
}

// Singleton caches for each operation type
const gitStatusCache = new AsyncTtlCache<string, GitStatusResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitBranchesCache = new AsyncTtlCache<string, GitBranchesResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitLogCache = new AsyncTtlCache<string, GitLogResult>(DEFAULT_GIT_CACHE_TTL_MS);

function statusCacheKey(profile: string, sessionId?: string): string {
  return sessionId ? `${profile}:${sessionId}` : profile;
}

/**
 * Cached git status: branch + changes for a profile's checkout, or for a
 * chat session's own worktree when sessionId is given.
 * Key: profile, or profile:sessionId
 */
export async function getGitStatusCached(profile: string, cwd: string, sessionId?: string): Promise<GitStatusResult> {
  return gitStatusCache.get(statusCacheKey(profile, sessionId), async () => {
    const { ok, out, err } = gitInDir(cwd, ['status', '--porcelain', '-b']);
    if (!ok) throw new Error(err);
    const lines = out.split('\n').filter(Boolean);
    const branchLine = lines[0]?.replace(/^## /, '') ?? '';
    // The `-b` header is `branch...origin/branch [ahead N, behind M]` (or
    // just `branch` with no upstream) -- strip the tracking/ahead-behind
    // suffix down to the branch name alone.
    const branch = branchLine.split('...')[0].split(' ')[0];
    const changes = lines.slice(1).map((l) => ({ status: l.slice(0, 2).trim(), path: l.slice(3) }));
    return { branch, changes, cwd };
  });
}

/**
 * Cached git branches: all branches + current for a profile's checkout.
 * Key: profile
 */
export async function getGitBranchesCached(profile: string, cwd: string): Promise<GitBranchesResult> {
  return gitBranchesCache.get(profile, async () => {
    const { ok, out, err } = gitInDir(cwd, ['branch', '-a', '--format=%(refname:short)']);
    if (!ok) throw new Error(err);
    const branches = out.split('\n').filter(Boolean);
    const current = gitInDir(cwd, ['branch', '--show-current']).out.trim();
    return { branches, current };
  });
}

/**
 * Cached git log: commits for a profile's checkout.
 * Key: profile:limit
 */
export async function getGitLogCached(profile: string, cwd: string, limit: number): Promise<GitLogResult> {
  const key = `${profile}:${limit}`;
  return gitLogCache.get(key, async () => {
    const safeLimit = Math.min(50, Math.max(1, limit));
    const { ok, out, err } = gitInDir(cwd, ['log', `--max-count=${safeLimit}`, '--pretty=format:%H|%h|%s|%an|%ar']);
    if (!ok) throw new Error(err);
    const commits = out.split('\n').filter(Boolean).map((l) => {
      const [hash, short, subject, author, ago] = l.split('|');
      return { hash, short, subject, author, ago };
    });
    return { commits };
  });
}

/**
 * Commits all changes or only the selected files. `git commit --only` keeps
 * unrelated staged files staged, which is the native partial-commit contract.
 * Not cached -- it's a mutation, not an observation --
 * but it drops the status/log cache entries it just invalidated so the next
 * strip refresh doesn't serve a stale pre-commit snapshot for the rest of
 * the TTL window.
 */
export async function commitGitChanges(
  profile: string,
  cwd: string,
  message: string,
  sessionId?: string,
  files?: string[]
): Promise<GitCommitResult> {
  let commit;
  if (files === undefined) {
    const add = gitInDir(cwd, ['add', '-A']);
    if (!add.ok) throw new Error(add.err || 'git add failed');
    commit = gitInDir(cwd, ['commit', '-m', message]);
  } else {
    const { selected, changes } = selectedChanges(cwd, files);
    const untracked = selected.filter(path => changes.untracked.has(path));
    if (untracked.length > 0) {
      const intent = gitInDir(cwd, ['add', '--intent-to-add', '--', ...untracked]);
      if (!intent.ok) throw new Error(intent.err || 'git add failed');
    }
    commit = gitInDir(cwd, ['commit', '--only', '-m', message, '--', ...selected]);
  }
  if (!commit.ok) throw new Error(commit.err || commit.out || 'git commit failed');
  gitStatusCache.delete(statusCacheKey(profile, sessionId));
  for (const key of gitLogCache.keys()) {
    if (key === profile || key.startsWith(`${profile}:`)) gitLogCache.delete(key);
  }
  const rev = gitInDir(cwd, ['rev-parse', 'HEAD']);
  return { hash: rev.ok ? rev.out.trim() : '' };
}
