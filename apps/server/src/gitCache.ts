/**
 * Git data caching module. Bounds CLI calls for git/gh/glab operations
 * with an in-memory TTL cache so that UI surfaces render instantly on
 * repeat and never hammer the provider.
 */
import { spawnSync } from 'node:child_process';
import { AsyncTtlCache } from './asyncTtlCache.js';

const DEFAULT_GIT_CACHE_TTL_MS = 15_000; // 15 seconds TTL for git data

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

function gitInDir(cwd: string, args: string[]): { ok: boolean; out: string; err: string } {
  const r = spawnSync('git', args, { cwd, encoding: 'utf8' });
  return { ok: r.status === 0, out: r.stdout ?? '', err: r.stderr ?? '' };
}

function cliInDir(bin: string, args: string[], cwd: string): { ok: boolean; out: string } {
  const r = spawnSync(bin, args, { cwd, encoding: 'utf8' });
  return { ok: r.status === 0, out: r.stdout ?? '' };
}

// Singleton caches for each operation type
const gitStatusCache = new AsyncTtlCache<string, GitStatusResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitBranchesCache = new AsyncTtlCache<string, GitBranchesResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitLogCache = new AsyncTtlCache<string, GitLogResult>(DEFAULT_GIT_CACHE_TTL_MS);
const gitPrsCache = new AsyncTtlCache<string, GitPrsResult>(DEFAULT_GIT_CACHE_TTL_MS);

/**
 * Cached git status: branch + changes for a profile's checkout.
 * Key: profile
 */
export async function getGitStatusCached(profile: string, cwd: string): Promise<GitStatusResult> {
  return gitStatusCache.get(profile, async () => {
    const { ok, out, err } = gitInDir(cwd, ['status', '--porcelain', '-b']);
    if (!ok) throw new Error(err);
    const lines = out.split('\n').filter(Boolean);
    const branch = lines[0]?.replace(/^## /, '') ?? '';
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

