/**
 * Thin HTTP client for apps/server's REST API (docs/openapi.yaml). This is
 * the whole point of this package (issue #862): translate MCP tool calls
 * into HTTP requests against the already-typed, already-documented HTTP
 * surface, rather than re-implementing `gah` CLI-calling logic a third time
 * (CLI -> gahCli.ts -> HTTP already exists; HTTP -> MCP is this file).
 */

import { Agent } from 'undici';

const BASE_URL = (process.env.GAH_SERVER_URL ?? 'http://127.0.0.1:3773').replace(/\/$/, '');
const SERVER_TOKEN = process.env.GAH_SERVER_TOKEN;
const DISPATCH_AGENT = new Agent({ headersTimeout: 7_260_000, bodyTimeout: 7_260_000 });

export class GahApiError extends Error {
  constructor(
    message: string,
    public readonly status: number
  ) {
    super(message);
    this.name = 'GahApiError';
  }
}

async function request<T>(
  method: 'GET' | 'POST',
  path: string,
  body?: unknown,
  waitForDispatch = false
): Promise<T> {
  const headers = new Headers();
  if (body !== undefined) headers.set('Content-Type', 'application/json');
  if (SERVER_TOKEN) headers.set('Authorization', `Bearer ${SERVER_TOKEN}`);
  const init: RequestInit & { dispatcher?: Agent } = {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    dispatcher: waitForDispatch ? DISPATCH_AGENT : undefined
  };
  const res = await fetch(`${BASE_URL}${path}`, init);
  const text = await res.text();
  let parsed: unknown;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    parsed = text;
  }
  if (!res.ok) {
    const message =
      parsed && typeof parsed === 'object' && 'message' in parsed
        ? String((parsed as { message: unknown }).message)
        : text || `HTTP ${res.status}`;
    throw new GahApiError(message, res.status);
  }
  return parsed as T;
}

function query(params: Record<string, string | number | boolean | undefined>): string {
  const usp = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) usp.set(key, String(value));
  }
  const qs = usp.toString();
  return qs ? `?${qs}` : '';
}

export const gah = {
  info: () => request('GET', '/api/info'),
  status: (profile: string) => request('GET', `/api/status${query({ profile })}`),
  quota: (profile: string, since?: string) => request('GET', `/api/quota${query({ profile, since })}`),
  usageRollup: (profile: string, days?: number) =>
    request('GET', `/api/usage/rollup${query({ profile, days })}`),
  doctor: (profile: string) => request('GET', `/api/doctor${query({ profile })}`),
  report: (profile?: string, since?: string, groupBy?: string) =>
    request('GET', `/api/report${query({ profile, since, groupBy })}`),
  profiles: () => request('GET', '/api/profiles'),
  workHistory: (workId: string) => request('GET', `/api/work/${encodeURIComponent(workId)}`),
  sync: (profile: string) => request('GET', `/api/sync${query({ profile })}`),
  ledgerSummary: (profile?: string, since?: string, groupBy?: string) =>
    request('GET', `/api/ledger/summary${query({ profile, since, groupBy })}`),
  ledgerClearAttempts: (profile: string, workId: string, dryRun?: boolean) =>
    request('POST', '/api/ledger/clear-attempts', { profile, workId, dryRun }),
  availability: () => request('GET', '/api/availability'),
  availabilityClear: (backend: string, backendInstance?: string, model?: string, quotaPool?: string) =>
    request('POST', '/api/availability/clear', { backend, backendInstance, model, quotaPool }),
  hold: (profile: string) => request('GET', `/api/hold${query({ profile })}`),
  holdSet: (profile: string, workId: string, reason?: string) =>
    request('POST', '/api/hold/set', { profile, workId, reason }),
  holdClear: (profile: string, workId: string) => request('POST', '/api/hold/clear', { profile, workId }),
  events: (profile: string, since?: string) => request('GET', `/api/events${query({ profile, since })}`),
  controllerActivity: (profile: string, since?: string) =>
    request('GET', `/api/controller-activity${query({ profile, since })}`),
  loopStatus: (profile: string) => request('GET', `/api/loop/status${query({ profile })}`),
  dispatch: (options: Record<string, unknown>) =>
    request('POST', '/api/dispatch', options, options.waitForCompletion === true)
};
