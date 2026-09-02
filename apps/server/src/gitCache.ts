/**
 * Git data caching module. Bounds CLI calls for git/gh/glab operations
 * with an in-memory TTL cache so that UI surfaces render instantly on
 * repeat and never hammer the provider.
 */
import { spawnSync } from 'node:child_process';
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

interface GitPrsResult {
  prs: Record<string, unknown>[];
  warning?: string;
}

interface GitCommitResult {
  hash: string;
}

function gitInDir(cwd: string, args: string[]): { ok: boolean; out: string; err: string } {
  const r = spawnSync('git', args, { cwd, encoding: 'utf8', timeout: SUBPROCESS_TIMEOUT_MS, maxBuffer: MAX_OUTPUT_BYTES });
  return { ok: r.status === 0, out: r.stdout ?? '', err: r.stderr ?? '' };
}

/** Shared by the PR-fetch path here and the `gh pr create`/`glab mr create`
 * mutation in server.ts, so both go through the same timeout/output bound. */
export function cliInDir(bin: string, args: string[], cwd: string): { ok: boolean; out: string } {
  const r = spawnSync(bin, args, { cwd, encoding: 'utf8', timeout: SUBPROCESS_TIMEOUT_MS, maxBuffer: MAX_OUTPUT_BYTES });
  return { ok: r.status === 0, out: r.stdout ?? '' };
}

// Singleton caches for each operation type
const gitStatusCache = new AsyncTtlCache<string, GitStatusResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitBranchesCache = new AsyncTtlCache<string, GitBranchesResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitLogCache = new AsyncTtlCache<string, GitLogResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitPrsCache = new AsyncTtlCache<string, GitPrsResult>(DEFAULT_GIT_CACHE_TTL_MS);

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
 * Cached git PRs: open PRs for a profile's checkout via gh/glab.
 * Key: profile
 */
export async function getGitPrsCached(
  profile: string,
  cwd: string,
  isGitLab: boolean
): Promise<GitPrsResult> {
  return gitPrsCache.get(profile, async () => {
    if (isGitLab) {
      const { ok, out } = cliInDir('glab', ['mr', 'list', '--output=json'], cwd);
      if (!ok) return { prs: [], warning: 'glab not available or no MRs' };
      try {
        return { prs: JSON.parse(out) };
      } catch {
        return { prs: [], warning: 'glab output parse error' };
      }
    } else {
      const { ok, out } = cliInDir('gh', ['pr', 'list', '--json', 'number,title,state,url,headRefName,isDraft'], cwd);
      if (!ok) return { prs: [], warning: 'gh not available or no PRs' };
      try {
        return { prs: JSON.parse(out) };
      } catch {
        return { prs: [], warning: 'gh output parse error' };
      }
    }
  });
}

/**
 * Commits staged + unstaged changes in a profile's checkout (`git add -A`
 * then `git commit`). Not cached -- it's a mutation, not an observation --
 * but it drops the status/log cache entries it just invalidated so the next
 * strip refresh doesn't serve a stale pre-commit snapshot for the rest of
 * the TTL window.
 */
export async function commitGitChanges(
  profile: string,
  cwd: string,
  message: string,
  sessionId?: string
): Promise<GitCommitResult> {
  const add = gitInDir(cwd, ['add', '-A']);
  if (!add.ok) throw new Error(add.err || 'git add failed');
  const commit = gitInDir(cwd, ['commit', '-m', message]);
  if (!commit.ok) throw new Error(commit.err || commit.out || 'git commit failed');
  gitStatusCache.delete(statusCacheKey(profile, sessionId));
  for (const key of gitLogCache.keys()) {
    if (key === profile || key.startsWith(`${profile}:`)) gitLogCache.delete(key);
  }
  const rev = gitInDir(cwd, ['rev-parse', 'HEAD']);
  return { hash: rev.ok ? rev.out.trim() : '' };
}
