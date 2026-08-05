/**
 * Thin HTTP client for the TencentDB Agent Memory Gateway (tdai-memory-gateway
 * systemd service, 127.0.0.1:8420 by default).
 *
 * This is the ONLY caller of the Gateway for manager chat -- deliberately not
 * also wired through each backend's own memory plugin (e.g. Hermes's
 * memory_tencentdb provider). One shared caller per profile is what makes
 * memory survive swapping which manager backend is actually running.
 */

const DEFAULT_BASE_URL = process.env.TDAI_GATEWAY_URL ?? 'http://127.0.0.1:8420';
const API_KEY = process.env.TDAI_GATEWAY_API_KEY;

export interface RecallResult {
  context: string;
  memoryCount: number;
}

export interface CaptureResult {
  l0Recorded: number;
}

/** One memory space per GAH profile -- every manager backend used for that
 * profile's chat shares it, so swapping backends doesn't lose context. */
export function sessionKeyForProfile(profile: string): string {
  return `gah:manager-chat:${profile}`;
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

/** Recall is explicitly non-critical (per the Gateway's own doc comments):
 * a failure here should degrade to "no extra context", not block the turn. */
export async function recall(profile: string, query: string): Promise<RecallResult> {
  try {
    const result = await postJson<{ context: string; memory_count: number; code: number; message: string }>(
      '/recall',
      { query, session_key: sessionKeyForProfile(profile) }
    );
    if (result.code !== 0) {
      console.warn(`[managerChat] recall degraded (code=${result.code}): ${result.message}`);
      return { context: '', memoryCount: 0 };
    }
    return { context: result.context, memoryCount: result.memory_count };
  } catch (error) {
    console.warn('[managerChat] recall failed, continuing without memory context:', error);
    return { context: '', memoryCount: 0 };
  }
}

/** Capture failures must not fail the turn either -- the user already has
 * their reply, losing it to a memory-write hiccup would be worse than a gap
 * in recall next time. */
export async function capture(
  profile: string,
  userContent: string,
  assistantContent: string
): Promise<CaptureResult> {
  try {
    const result = await postJson<{ l0_recorded: number; scheduler_notified: boolean }>('/capture', {
      user_content: userContent,
      assistant_content: assistantContent,
      session_key: sessionKeyForProfile(profile)
    });
    return { l0Recorded: result.l0_recorded };
  } catch (error) {
    console.warn('[managerChat] capture failed (turn is not blocked on this):', error);
    return { l0Recorded: 0 };
  }
}
