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
  ReportSeriesData,
  ReportGroupBy,
  LedgerEntry,
  ControllerEvent,
  ProfileSummary
} from '@git-agent-harness/contracts';

const SERVER_URL =
  (import.meta as unknown as { env: { VITE_SERVER_URL?: string } }).env?.VITE_SERVER_URL ||
  window.location.origin;

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

export interface ProfileAddData {
  name: string;
  display_name: string;
  repo_id: string;
  provider: string;
  repo: string;
  local_path: string;
  artifact_root: string;
  default_target_branch?: string;
  provider_api_base?: string;
  provider_project_id?: string;
  openhands_args?: string[];
  codex_args?: string[];
  codex_path?: string;
  claude_args?: string[];
  claude_path?: string;
  agy_path?: string;
  vibe_args?: string[];
  vibe_path?: string;
  opencode_args?: string[];
  opencode_path?: string;
  agy_second_home?: string;
  notify_command?: string;
  policy_path?: string;
  env_file?: string;
  env_file_prod?: string;
  validation_commands?: string[];
  auto_fix_commands?: string[];
}

export interface ProfileUpdateData {
  display_name?: string;
  repo_id?: string;
  provider?: string;
  repo?: string;
  local_path?: string;
  artifact_root?: string;
  default_target_branch?: string;
  provider_api_base?: string | null;
  provider_project_id?: string | null;
  openhands_args?: string[];
  codex_args?: string[];
  codex_path?: string | null;
  claude_args?: string[];
  claude_path?: string | null;
  agy_path?: string | null;
  vibe_args?: string[];
  vibe_path?: string | null;
  opencode_args?: string[];
  opencode_path?: string | null;
  agy_second_home?: string | null;
  notify_command?: string | null;
  policy_path?: string | null;
  env_file?: string | null;
  env_file_prod?: string | null;
  validation_commands?: string[];
  auto_fix_commands?: string[];
  clear?: string[];
}

export interface ProfileRemoveParams {
  force?: boolean;
}

function profileRemoveParamsToRecord(params?: ProfileRemoveParams): Record<string, string | undefined> {
  if (!params) return {};
  const result: Record<string, string | undefined> = {};
  if (params.force !== undefined) {
    result.force = params.force ? 'true' : 'false';
  }
  return result;
}

export interface GahDataSource {
  getStatus(profile?: string): Promise<StatusSnapshot>;
  getReport(params?: { profile?: string; since?: string; groupBy?: ReportGroupBy }): Promise<ReportData>;
  getReportSeries(params?: { profile?: string; since?: string; bucket?: string }): Promise<ReportSeriesData>;
  getWorkTimeline(workId: string): Promise<LedgerEntry[]>;
  getEvents(params?: { profile?: string; since?: string }): Promise<ControllerEvent[]>;
  getProfiles(): Promise<ProfileSummary[]>;
  addProfile(data: ProfileAddData): Promise<{ success: boolean; message: string }>;
  updateProfile(name: string, data: ProfileUpdateData): Promise<{ success: boolean; message: string }>;
  removeProfile(name: string, params?: ProfileRemoveParams): Promise<{ success: boolean; message: string }>;
}

async function postJson<T, U>(path: string, body: U): Promise<T> {
  const url = new URL(path, SERVER_URL);
  const res = await fetch(url.toString(), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
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

async function patchJson<T, U>(path: string, body: U): Promise<T> {
  const url = new URL(path, SERVER_URL);
  const res = await fetch(url.toString(), {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
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

async function deleteJson<T>(path: string, params?: Record<string, string | undefined>): Promise<T> {
  const url = new URL(path, SERVER_URL);
  if (params) {
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined) url.searchParams.set(key, value);
    }
  }
  const res = await fetch(url.toString(), { method: 'DELETE' });
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
  getReportSeries(params = {}) {
    return getJson<ReportSeriesData>('/api/report/series', {
      profile: params.profile,
      since: params.since,
      bucket: params.bucket
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
  },
  getProfiles() {
    return getJson<ProfileSummary[]>('/api/profiles');
  },
  addProfile(data) {
    return postJson<{ success: boolean; message: string }, ProfileAddData>('/api/profiles', data);
  },
  updateProfile(name, data) {
    return patchJson<{ success: boolean; message: string }, ProfileUpdateData>(`/api/profiles/${encodeURIComponent(name)}`, data);
  },
  removeProfile(name, params = {}) {
    return deleteJson<{ success: boolean; message: string }>(`/api/profiles/${encodeURIComponent(name)}`, profileRemoveParamsToRecord(params));
  }
};
