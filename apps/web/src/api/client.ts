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
  QuotaSnapshot,
  ReportData,
  ReportSeriesData,
  ReportGroupBy,
  LedgerEntry,
  ControllerEvent,
  ControllerActivity,
  ProfileSummary,
  WakeAutonomyValue,
  ConfigSummary,
  ConfigProfileSummary,
  DoctorSnapshot,
  ConfigSetData,
  ManagerChatSettingsSummary,
  ManagerCommandInfo,
  ManagerModelsSummary,
  ManagerChatSettingsUpdate,
  GatewaySettingsSummary,
  GatewaySettingsUpdate,
  ProjectImportData,
  ProjectImportResult,
  ChatSessionSummary,
  ChatNodeInfo,
  ChatPreviewInfo
} from '@git-agent-harness/contracts';

const SERVER_URL =
  (import.meta as unknown as { env: { VITE_SERVER_URL?: string } }).env?.VITE_SERVER_URL ||
  (typeof window !== 'undefined' ? window.location.origin : 'http://localhost:3000');

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
  /** Max concurrent tickets `gah loop` may run for this profile. */
  max_parallel_workers?: number;
  /** Validation command timeout in seconds. */
  validation_timeout_seconds?: number;
  /** Manager-wake autonomy: 'off' | 'review_only' | 'full'. */
  manager_wake_autonomy?: WakeAutonomyValue;
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
  /** Validation command timeout in seconds. */
  validation_timeout_seconds?: number | null;
  max_parallel_workers?: number;
  manager_wake_autonomy?: WakeAutonomyValue;
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

export interface LoopStatus {
  running: boolean;
  pid?: number;
  startedAt?: string;
}

export interface StartLoopResult {
  started: boolean;
  pid?: number;
  alreadyRunning?: boolean;
  error?: string;
}

export interface StopLoopResult {
  stopped: boolean;
  error?: string;
}

export interface GahDataSource {
  getStatus(profile?: string): Promise<StatusSnapshot>;
  getQuota(params?: { profile?: string; since?: string }): Promise<QuotaSnapshot>;
  getDoctor(profile?: string): Promise<DoctorSnapshot>;
  getReport(params?: { profile?: string; since?: string; groupBy?: ReportGroupBy }): Promise<ReportData>;
  getReportSeries(params?: { profile?: string; since?: string; bucket?: string }): Promise<ReportSeriesData>;
  getWorkTimeline(workId: string): Promise<LedgerEntry[]>;
  getEvents(params?: { profile?: string; since?: string }): Promise<ControllerEvent[]>;
  getControllerActivity(params?: { profile?: string; since?: string }): Promise<ControllerActivity[]>;
  getProfiles(): Promise<ProfileSummary[]>;
  getProjects(): Promise<ProfileSummary[]>;
  addProject(profile: string): Promise<ProfileSummary>;
  removeProject(profile: string): Promise<{ removed: boolean }>;
  importProject(data: ProjectImportData): Promise<ProjectImportResult>;
  addProfile(data: ProfileAddData): Promise<{ success: boolean; message: string }>;
  updateProfile(name: string, data: ProfileUpdateData): Promise<{ success: boolean; message: string }>;
  removeProfile(name: string, params?: ProfileRemoveParams): Promise<{ success: boolean; message: string }>;
  getLoopStatus(profile?: string): Promise<LoopStatus>;
  startLoop(profile: string): Promise<StartLoopResult>;
  stopLoop(profile: string): Promise<StopLoopResult>;
  getConfig(): Promise<ConfigSummary>;
  getProfileConfig(profile: string): Promise<ConfigProfileSummary>;
  setConfig(data: ConfigSetData): Promise<{ success: boolean; message: string }>;
  getManagerChatSettings(): Promise<ManagerChatSettingsSummary>;
  setManagerChatSettings(data: ManagerChatSettingsUpdate): Promise<{ success: boolean }>;
  getGatewaySettings(): Promise<GatewaySettingsSummary>;
  updateGatewaySettings(data: GatewaySettingsUpdate): Promise<GatewaySettingsSummary>;
  recallContext(profile: string, query: string): Promise<{ context: string; memoryCount: number }>;
  getGitStatus(profile: string): Promise<{ branch: string; changes: { status: string; path: string }[]; cwd: string }>;
  getGitBranches(profile: string): Promise<{ branches: string[]; current: string }>;
  getGitLog(profile: string, limit?: number): Promise<{ commits: { hash: string; short: string; subject: string; author: string; ago: string }[] }>;
  getGitPrs(profile: string): Promise<{ prs: Record<string, unknown>[]; warning?: string }>;
  createGitPr(profile: string, data: { title: string; body?: string; base?: string; draft?: boolean }): Promise<{ url: string }>;
  getManagerChatCommands(profile: string): Promise<{ commands: ManagerCommandInfo[] }>;
  getManagerChatModels(profile: string): Promise<ManagerModelsSummary>;
  setManagerChatModel(profile: string, modelId: string): Promise<{ success: boolean }>;
  getChatSessions(profile: string): Promise<{ sessions: ChatSessionSummary[] }>;
  createChatSession(profile: string, backend?: string, model?: string | null, title?: string): Promise<ChatSessionSummary>;
  updateChatSession(profile: string, sessionId: string, patch: { backend?: string; model?: string | null; title?: string }): Promise<ChatSessionSummary>;
  archiveChatSession(profile: string, sessionId: string): Promise<ChatSessionSummary>;
  getChatNodes(): Promise<{ nodes: ChatNodeInfo[] }>;
  getManagerChatModelsForBackend(profile: string, backend: string): Promise<ManagerModelsSummary>;
  getChatPreview(profile: string, sessionId: string): Promise<{ preview: ChatPreviewInfo | null }>;
  setChatPreview(profile: string, sessionId: string, port: number | null): Promise<{ preview: ChatPreviewInfo | null }>;
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

async function putJson<T, U>(path: string, body: U): Promise<T> {
  const url = new URL(path, SERVER_URL);
  const res = await fetch(url.toString(), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const b = await res.json();
      if (typeof b?.message === 'string') message = b.message;
    } catch { /* fall back to status text */ }
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
  getQuota(params = {}) {
    return getJson<QuotaSnapshot>('/api/quota', {
      profile: params.profile,
      since: params.since
    });
  },
  getDoctor(profile) {
    return getJson<DoctorSnapshot>('/api/doctor', { profile });
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
  getControllerActivity(params = {}) {
    return getJson<ControllerActivity[]>('/api/controller-activity', {
      profile: params.profile,
      since: params.since
    });
  },
  getProfiles() {
    return getJson<ProfileSummary[]>('/api/profiles');
  },
  getProjects() {
    return getJson<ProfileSummary[]>('/api/projects');
  },
  addProject(profile) {
    return postJson<ProfileSummary, { profile: string }>('/api/projects', { profile });
  },
  removeProject(profile) {
    return deleteJson<{ removed: boolean }>(`/api/projects/${encodeURIComponent(profile)}`);
  },
  importProject(data) {
    return postJson<ProjectImportResult, ProjectImportData>('/api/projects/import', data);
  },
  addProfile(data) {
    return postJson<{ success: boolean; message: string }, ProfileAddData>('/api/profiles', data);
  },
  updateProfile(name, data) {
    return patchJson<{ success: boolean; message: string }, ProfileUpdateData>(`/api/profiles/${encodeURIComponent(name)}`, data);
  },
  removeProfile(name, params = {}) {
    return deleteJson<{ success: boolean; message: string }>(`/api/profiles/${encodeURIComponent(name)}`, profileRemoveParamsToRecord(params));
  },
  getLoopStatus(profile) {
    return getJson<LoopStatus>('/api/loop/status', { profile });
  },
  async startLoop(profile) {
    // Unlike the other POSTs above, a non-2xx here (409) still carries a
    // meaningful body -- "already running", with the existing pid -- not
    // just an error to throw, so this reads the JSON directly instead of
    // going through postJson's throw-on-!ok.
    const url = new URL('/api/loop/start', SERVER_URL);
    const res = await fetch(url.toString(), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile })
    });
    return (await res.json()) as StartLoopResult;
  },
  async stopLoop(profile) {
    const url = new URL('/api/loop/stop', SERVER_URL);
    const res = await fetch(url.toString(), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile })
    });
    return (await res.json()) as StopLoopResult;
  },
  async getConfig() {
    return getJson<ConfigSummary>('/api/config');
  },
  getProfileConfig(profile) {
    return getJson<ConfigProfileSummary>('/api/config/effective', { profile });
  },
  async setConfig(data) {
    return postJson<{ success: boolean; message: string }, ConfigSetData>('/api/config', data);
  },
  getManagerChatSettings() {
    return getJson<ManagerChatSettingsSummary>('/api/manager-chat/settings');
  },
  setManagerChatSettings(data) {
    return postJson<{ success: boolean }, ManagerChatSettingsUpdate>('/api/manager-chat/settings', data);
  },
  getGatewaySettings() {
    return getJson<GatewaySettingsSummary>('/api/settings/gateway');
  },
  updateGatewaySettings(data) {
    return putJson<GatewaySettingsSummary, GatewaySettingsUpdate>('/api/settings/gateway', data);
  },
  recallContext(profile, query) {
    return postJson<{ context: string; memoryCount: number }, { profile: string; query: string }>(
      '/api/context/recall',
      { profile, query }
    );
  },
  getGitStatus(profile) {
    return getJson('/api/git/status', { profile });
  },
  getGitBranches(profile) {
    return getJson('/api/git/branches', { profile });
  },
  getGitLog(profile, limit) {
    return getJson('/api/git/log', { profile, limit: limit?.toString() });
  },
  getGitPrs(profile) {
    return getJson('/api/git/prs', { profile });
  },
  createGitPr(profile, data) {
    return postJson(`/api/git/pr?profile=${encodeURIComponent(profile)}`, data);
  },
  getManagerChatCommands(profile) {
    return getJson<{ commands: ManagerCommandInfo[] }>('/api/manager-chat/commands', { profile });
  },
  getManagerChatModels(profile) {
    return getJson<ManagerModelsSummary>('/api/manager-chat/models', { profile });
  },
  setManagerChatModel(profile, modelId) {
    return postJson<{ success: boolean }, { profile: string; modelId: string }>('/api/manager-chat/model', { profile, modelId });
  },
  getChatSessions(profile) {
    return getJson<{ sessions: ChatSessionSummary[] }>('/api/manager-chat/sessions', { profile });
  },
  createChatSession(profile, backend, model, title) {
    return postJson<ChatSessionSummary, { profile: string; backend?: string; model?: string | null; title?: string }>('/api/manager-chat/sessions', { profile, backend, model, title });
  },
  updateChatSession(profile, sessionId, patch) {
    return postJson<ChatSessionSummary, { profile: string; sessionId: string } & { backend?: string; model?: string | null; title?: string }>('/api/manager-chat/sessions/update', { profile, sessionId, ...patch });
  },
  archiveChatSession(profile, sessionId) {
    return postJson<ChatSessionSummary, { profile: string; sessionId: string }>('/api/manager-chat/sessions/archive', { profile, sessionId });
  },
  getChatNodes() {
    return getJson<{ nodes: ChatNodeInfo[] }>('/api/manager-chat/nodes');
  },
  getManagerChatModelsForBackend(profile, backend) {
    return getJson<ManagerModelsSummary>('/api/manager-chat/models', { profile, backend });
  },
  getChatPreview(profile, sessionId) {
    return getJson<{ preview: ChatPreviewInfo | null }>('/api/manager-chat/preview', { profile, sessionId });
  },
  setChatPreview(profile, sessionId, port) {
    return postJson<{ preview: ChatPreviewInfo | null }, { profile: string; sessionId: string; port: number | null }>('/api/manager-chat/preview/set', { profile, sessionId, port });
  }
};
