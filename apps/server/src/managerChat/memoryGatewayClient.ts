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
import { effectiveGatewayUrl, effectiveGatewayApiKey } from '../gatewaySettingsStore.js';

// Read fresh per call: lets tests substitute, and lets runtime config changes
// (PUT /api/settings/gateway) take effect without a server restart.
export function gatewayBaseUrl(): string {
  return effectiveGatewayUrl();
}
export function gatewayApiKey(): string | undefined {
  return effectiveGatewayApiKey();
}

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

/** Same normalization Rust's `work_claim::normalize_work_identity` applies
 * (`src/work_claim.rs`), mirrored here so "#71", "71", and "TICKET-071" all
 * resolve to the same session key regardless of which caller/language names
 * the ticket -- a repair agent and a review agent must land in the same
 * memory space for the same ticket. */
function normalizeTicketId(raw: string): string {
  const trimmed = raw.trim();
  const digits = trimmed.replace(/^#/, '').replace(/^TICKET-/i, '');
  if (/^\d+$/.test(digits)) {
    return `#${digits.replace(/^0+(?=\d)/, '')}`;
  }
  return trimmed;
}

/** Issue #885: leaf scope for one ticket's dispatch-side memory (repair,
 * review, ...), namespaced under the same project key `sessionKeyForProfile`
 * uses so both live in one gateway store without colliding. */
export async function sessionKeyForTicket(profile: string, ticketId: string): Promise<string> {
  return `gah:worker:${await resolveProjectKey(profile)}:${normalizeTicketId(ticketId)}`;
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const apiKey = gatewayApiKey();
  if (apiKey) headers.Authorization = `Bearer ${apiKey}`;
  const res = await fetch(`${gatewayBaseUrl()}${path}`, {
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

async function recallForKey(sessionKey: string, query: string): Promise<RecallResult> {
  const result = await postJson<{ context: string; memory_count: number; code: number; message: string }>(
    '/recall',
    { query, session_key: sessionKey }
  );
  if (result.code !== 0) {
    throw new Error(`memory gateway recall degraded (code=${result.code}): ${result.message}`);
  }
  return { context: result.context, memoryCount: result.memory_count };
}

async function captureForKey(
  sessionKey: string,
  userContent: string,
  assistantContent: string
): Promise<CaptureResult> {
  const result = await postJson<{ l0_recorded: number; scheduler_notified: boolean }>('/capture', {
    user_content: userContent,
    assistant_content: assistantContent,
    session_key: sessionKey
  });
  return { l0Recorded: result.l0_recorded };
}

export async function recall(profile: string, query: string): Promise<RecallResult> {
  return recallForKey(await sessionKeyForProfile(profile), query);
}

export async function capture(
  profile: string,
  userContent: string,
  assistantContent: string
): Promise<CaptureResult> {
  return captureForKey(await sessionKeyForProfile(profile), userContent, assistantContent);
}

/** Ticket-scoped recall for repair/review agents -- callers never see key
 * construction, gateway URL, or auth (issue #885). Wiring this into real
 * dispatch attempts is issue #830's scope, gated on #878 (manager-chat's
 * hard-block-on-failure fix) landing first. */
export async function recallForTicket(
  profile: string,
  ticketId: string,
  query: string
): Promise<RecallResult> {
  return recallForKey(await sessionKeyForTicket(profile, ticketId), query);
}

export async function captureForTicket(
  profile: string,
  ticketId: string,
  userContent: string,
  assistantContent: string
): Promise<CaptureResult> {
  return captureForKey(await sessionKeyForTicket(profile, ticketId), userContent, assistantContent);
}

/** Backs both /clear and compaction-like commands (see ManagerChatManager's
 * isCompactionCommand): flushes buffered L0 conversation into the gateway's
 * L1/L2 extraction pipeline immediately (core.handleSessionEnd ->
 * scheduler.flushSession), instead of waiting for the pipeline's own idle
 * timeout.
 *
 * CONTRACT (issue #958): a successful `/session/end` is a barrier for that
 * session. It MUST NOT return while an L1 extraction for the session is
 * queued or in flight -- an immediate `/recall` after this call must observe
 * the committed memory, or the next chat can recall a superseded value
 * (repro: store B after A, /reset, fresh chat recalls stale A).
 *
 * The root behavior lives in the gateway (`MemoryCore/src/gateway/server.ts`
 * `handleSessionEnd` -> `stateful-pipeline-manager.ts` `flushSession`, which
 * only ENQUEUES a flush task and returns while the in-process
 * `PipelineWorker` still has to run it). GAH deliberately does NOT paper
 * over that with a delay or polling loop; the gateway must await the queued
 * flush task's completion before returning `{ flushed: true }`. */
export async function flushSession(profile: string): Promise<boolean> {
  const result = await postJson<{ flushed: boolean }>('/session/end', {
    session_key: await sessionKeyForProfile(profile)
  });
  return result.flushed === true;
}
