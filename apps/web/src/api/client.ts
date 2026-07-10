/**
 * Typed data-source client for GAH's pull-data REST endpoints.
 *
 * Deliberately plain fetch() over HTTP, not a Tauri `invoke()` call and not
 * threaded through the WebSocket. This is what keeps one shared frontend
 * working across web, Tauri desktop, and (eventually) Tauri mobile: the
 * server already listens on a normal HTTP port (same port as the
 * WebSocket, see apps/server/src/bin.ts) with CORS enabled, and a Tauri
 * webview can fetch() a local HTTP server exactly like a browser tab can --
 * no native bridge needed for this data. If a genuine Tauri-only data path
 * shows up later (e.g. reading a file the web build can't reach), give it
 * its own function here rather than routing everything through invoke().
 *
 * Live/push data (sessions starting/stopping, provider status changes)
 * stays on the WebSocket (see ws/WebSocketContext.tsx) -- this client only
 * covers on-demand pulls: status snapshot, backend/model report, one work
 * item's attempt history, the controller event stream.
 */
import type {
  StatusSnapshot,
  ReportData,
  ReportGroupBy,
  LedgerEntry,
  ControllerEvent
} from '@git-agent-harness/contracts';

const SERVER_URL =
  (import.meta as unknown as { env: { VITE_SERVER_URL?: string } }).env?.VITE_SERVER_URL ||
  'http://localhost:3773';

export class GahApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly endpoint: string
  ) {
    super(message);
    this.name = 'GahApiError';
  }
}

async function getJson<T>(path: string, params?: Record<string, string | undefined>): Promise<T> {
  const url = new URL(path, SERVER_URL);
  if (params) {
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined) url.searchParams.set(key, value);
    }
  }
  const res = await fetch(url.toString());
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = await res.json();
      if (typeof body?.message === 'string') message = body.message;
    } catch {
      // response body wasn't JSON -- fall back to the status text above
    }
    throw new GahApiError(message, res.status, path);
  }
  return (await res.json()) as T;
}

export interface GahDataSource {
  getStatus(profile?: string): Promise<StatusSnapshot>;
  getReport(params?: { profile?: string; since?: string; groupBy?: ReportGroupBy }): Promise<ReportData>;
  getWorkTimeline(workId: string): Promise<LedgerEntry[]>;
  getEvents(params?: { profile?: string; since?: string }): Promise<ControllerEvent[]>;
}

export const gahApi: GahDataSource = {
  getStatus(profile) {
    return getJson<StatusSnapshot>('/api/status', { profile });
  },
  getReport(params = {}) {
    return getJson<ReportData>('/api/report', {
      profile: params.profile,
      since: params.since,
      groupBy: params.groupBy
    });
  },
  getWorkTimeline(workId) {
    return getJson<LedgerEntry[]>(`/api/work/${encodeURIComponent(workId)}`);
  },
  getEvents(params = {}) {
    return getJson<ControllerEvent[]>('/api/events', {
      profile: params.profile,
      since: params.since
    });
  }
};
