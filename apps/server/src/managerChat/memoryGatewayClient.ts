/**
 * Thin HTTP client for the TencentDB Agent Memory Gateway (tdai-memory-gateway
 * systemd service, 127.0.0.1:8420 by default).
 *
 * This is the ONLY caller of the Gateway for manager chat -- deliberately not
 * also wired through each backend's own memory plugin (e.g. Hermes's
 * memory_tencentdb provider). One shared caller per profile is what makes
 * memory survive swapping which manager backend is actually running.
 *
 * The gateway is a required dependency, not a soft enhancement: recall,
 * capture, and flushSession all hard-block the turn on failure (throw)
 * rather than degrading silently. This is a deliberate policy choice (2026-08
 * design pass, issue #849) -- silent degradation was masking real gateway
 * outages instead of surfacing them.
 */

import { spawnSync } from 'node:child_process';
import { runProfileList } from '../gahCli.js';
import { AsyncTtlCache } from '../asyncTtlCache.js';

const DEFAULT_BASE_URL = process.env.TDAI_GATEWAY_URL ?? 'http://127.0.0.1:8420';
const API_KEY = process.env.TDAI_GATEWAY_API_KEY;

export interface RecallResult {
  context: string;
  memoryCount: number;
}

export interface CaptureResult {
  l0Recorded: number;
}

// GAH profile names get renamed in practice (this project's own history:
// worldcup-props -> sportsball -> sportsball-bets, one project, three
// names) and a rename must not orphan captured memory. Key on the profile's
// git remote URL instead -- stable across profile/project renames as long
// as the local checkout's remote isn't repointed. Resolution failures (no
// checkout yet, not a git repo) fall back to the raw profile name: this is
// a different failure class from the gateway being unreachable, so it
// degrades here rather than hard-blocking like recall/capture/flushSession do.
const PROJECT_KEY_CACHE_TTL_MS = 24 * 60 * 60 * 1000;
const projectKeyCache = new AsyncTtlCache<string, string>(PROJECT_KEY_CACHE_TTL_MS);

/** Normalizes a git remote URL into a stable identity: strips scheme,
 * credentials, and a trailing .git, and converts scp-style host:path to
 * host/path. Doesn't need to handle every possible git URL shape -- just
 * needs to be stable across http/ssh variants of the same remote. */
export function normalizeRemoteUrl(raw: string): string {
  let s = raw.trim().toLowerCase();
  s = s.replace(/^[a-z][a-z0-9+.-]*:\/\//, '');
  s = s.replace(/^[^@/]+@/, '');
  s = s.replace(/:(?!\d+\/)/, '/');
  s = s.replace(/\.git$/, '');
  return s.replace(/\/+$/, '');
}

async function resolveProjectKey(profile: string): Promise<string> {
  return projectKeyCache.get(profile, async () => {
    try {
      const profiles = await runProfileList();
      const localPath = profiles.find((p) => p.name === profile)?.local_path;
      if (!localPath) return profile;
      const result = spawnSync('git', ['-C', localPath, 'remote', 'get-url', 'origin'], { encoding: 'utf8' });
      if (result.status !== 0 || !result.stdout.trim()) return profile;
      return normalizeRemoteUrl(result.stdout);
    } catch {
      return profile;
    }
  });
}

/** One memory space per project, namespaced for the eventual dispatch-backend
 * side (#830: `gah:worker:{project}:{ticket}`) to share the same store
 * without colliding. Every manager backend used for that profile's chat
 * shares this key, so swapping backends doesn't lose context. */
export async function sessionKeyForProfile(profile: string): Promise<string> {
  return `gah:manager:${await resolveProjectKey(profile)}`;
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (API_KEY) headers.Authorization = `Bearer ${API_KEY}`;
  const res = await fetch(`${DEFAULT_BASE_URL}${path}`, {
    method: 'POST',
    headers,
    body: JSON.stringify(body)
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`memory gateway ${path} returned ${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export async function recall(profile: string, query: string): Promise<RecallResult> {
  const result = await postJson<{ context: string; memory_count: number; code: number; message: string }>(
    '/recall',
    { query, session_key: await sessionKeyForProfile(profile) }
  );
  if (result.code !== 0) {
    throw new Error(`memory gateway recall degraded (code=${result.code}): ${result.message}`);
  }
  return { context: result.context, memoryCount: result.memory_count };
}

export async function capture(
  profile: string,
  userContent: string,
  assistantContent: string
): Promise<CaptureResult> {
  const result = await postJson<{ l0_recorded: number; scheduler_notified: boolean }>('/capture', {
    user_content: userContent,
    assistant_content: assistantContent,
    session_key: await sessionKeyForProfile(profile)
  });
  return { l0Recorded: result.l0_recorded };
}

/** Backs both /clear and compaction-like commands (see ManagerChatManager's
 * isCompactionCommand): flushes buffered L0 conversation into the gateway's
 * L1/L2 extraction pipeline immediately (core.handleSessionEnd ->
 * scheduler.flushSession), instead of waiting for the pipeline's own idle
 * timeout. */
export async function flushSession(profile: string): Promise<boolean> {
  const result = await postJson<{ flushed: boolean }>('/session/end', {
    session_key: await sessionKeyForProfile(profile)
  });
  return result.flushed === true;
}
