/**
 * Stateful manager-chat control plane for browser development and Playwright.
 *
 * This file deliberately lives outside src/: production's tsc build never
 * emits or imports it. It does not import the production server, manager chat
 * adapters, provider registry, git/worktree helpers, or state stores. The only
 * mutable data is the in-memory MockState below.
 */
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import cors from 'cors';
import express, { type Response } from 'express';
import { WebSocket, WebSocketServer } from 'ws';
import type {
  AdminUpdatePendingInfo,
  AdminUpdateState,
  ChatNodeInfo,
  ChatIssueStartResult,
  ChatIssueSummary,
  ChatPrStartResult,
  ChatPrSummary,
  ChatPreviewInfo,
  ChatReclaimResult,
  ChatSessionProjectGroup,
  ChatSessionSummary,
  ChatSessionView,
  ChatTranscriptTurn,
  ClientMessage,
  ConfigSummary,
  GatewayBootstrapCommand,
  GatewaySettingsSummary,
  SettingsConfigProfileSummary,
  DoctorSnapshot,
  LedgerEntry,
  ManagerBackendInfo,
  ManagerChatSettingsSummary,
  ManagerCommandInfo,
  ManagerModelsSummary,
  ProfileSummary,
  ProjectImportResult,
  QuotaSnapshot,
  ReportData,
  ReportSeriesData,
  ServerMessage,
  SkillSummary,
  StatusSnapshot,
  UsageRollupSummary
} from '@git-agent-harness/contracts';

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE_ROOT = resolve(HERE, '../tests/fixtures/gah/responses');
const FIXED_NOW = 1_700_000_000_000;

function readFixture<T>(name: string): T {
  return JSON.parse(readFileSync(resolve(FIXTURE_ROOT, name), 'utf8')) as T;
}

// Reuse the same captured CLI data as fixtureGahHarness.ts. The Rust/TS
// contract drift tests already own these committed recordings; chat-specific
// payloads below use `satisfies` directly against the production contracts.
const STATUS_FIXTURE = readFixture<StatusSnapshot>('status.json');
const QUOTA_FIXTURE = readFixture<QuotaSnapshot>('quota.json');
const REPORT_FIXTURE = readFixture<ReportData>('report.json');
const PROFILE_FIXTURE = readFixture<ProfileSummary[]>('profile-list.json');

export const MOCK_SCENARIOS = {
  normal: {
    family: 'streaming',
    description: 'Multi-chunk reply with a pending/completed tool call and terminal reply.'
  },
  'slow-cancel-steer': {
    family: 'permission/cancel/steer',
    description: 'A slow turn accepts steering, rejects a named steer, and waits for cancellation.'
  },
  'reconnect-stream': {
    family: 'streaming/reconnect',
    description: 'Drops the WebSocket mid-turn and resumes chunks after reconnect/history restore.'
  },
  'reconnect-permission': {
    family: 'permission/reconnect',
    description: 'Drops the WebSocket with a durable pending permission that is actionable after reconnect.'
  },
  'archive-success': {
    family: 'archive',
    description: 'Archives the seeded session in memory.'
  },
  'archive-failure': {
    family: 'archive',
    description: 'Returns a deterministic archive failure without mutating the session.'
  },
  'preview-unavailable': {
    family: 'preview',
    description: 'The seeded session has no preview.'
  },
  'preview-available': {
    family: 'preview',
    description: 'The seeded session has a live preview served by this mock process.'
  },
  'preview-blocked': {
    family: 'preview',
    description: 'Returns an unsafe preview URL so the real UI blocks it.'
  },
  'preview-error': {
    family: 'preview',
    description: 'Preview mutation returns a deterministic REST error.'
  },
  'models-success': {
    family: 'backend/models',
    description: 'Codex-shaped models and provider-owned reasoning efforts.'
  },
  'models-empty': {
    family: 'backend/models',
    description: 'A backend succeeds with no selectable models or reasoning efforts.'
  },
  'models-delayed': {
    family: 'backend/models',
    description: 'Model discovery succeeds after a deterministic delay.'
  },
  'models-failure': {
    family: 'backend/models',
    description: 'Model discovery returns a deterministic 502.'
  },
  'models-agy': {
    family: 'backend/models',
    description: 'AGY headless-backend shape: implemented backend with no live config options.'
  },
  'rest-error': {
    family: 'errors',
    description: 'The chat session-list REST request returns a deterministic 503.'
  },
  'ws-error': {
    family: 'errors',
    description: 'A manager.chat.send receives the production WebSocket error shape.'
  }
} as const;

export type MockScenarioName = keyof typeof MOCK_SCENARIOS;

export function isMockScenarioName(value: unknown): value is MockScenarioName {
  return typeof value === 'string' && value in MOCK_SCENARIOS;
}

interface ConversationState {
  turns: ChatTranscriptTurn[];
  cursor: number;
  streaming: NonNullable<ChatSessionView['streaming']> | null;
  permission: NonNullable<ChatSessionView['permission']> | null;
  requestId: string | null;
  steers: string[];
  reconnectResumed: boolean;
}

interface MockState {
  scenario: MockScenarioName;
  reset: number;
  connectionCount: number;
  settings: ManagerChatSettingsSummary;
  profiles: ProfileSummary[];
  config: ConfigSummary;
  gateway: GatewaySettingsSummary;
  skills: SkillSummary[];
  loopRunning: boolean;
  adminUpdate: AdminUpdateState;
  gitPrs: ChatPrSummary[];
  selectedModels: Record<string, string | null>;
  selectedEfforts: Record<string, string | null>;
  sessions: Map<string, ChatSessionSummary>;
  conversations: Map<string, ConversationState>;
  previews: Map<string, ChatPreviewInfo>;
  timers: Set<ReturnType<typeof setTimeout>>;
}

export interface MockControlPlaneOptions {
  scenario?: MockScenarioName;
  previewOrigin?: string;
}

export interface RunningMockControlPlane {
  baseUrl: string;
  wsUrl: string;
  scenario(): MockScenarioName;
  close(): Promise<void>;
}

const BACKENDS = [
  { id: 'hermes', displayName: 'Hermes', implemented: true },
  { id: 'codex', displayName: 'Codex', implemented: true },
  { id: 'claude', displayName: 'Claude', implemented: true },
  { id: 'opencode', displayName: 'OpenCode', implemented: true },
  { id: 'agy', displayName: 'AGY', implemented: true },
  { id: 'vibe', displayName: 'Vibe', implemented: true }
] satisfies ManagerBackendInfo[];

const MOCK_PROFILES = PROFILE_FIXTURE satisfies ProfileSummary[];

const MOCK_GATEWAY = {
  url: 'http://127.0.0.1:8420',
  apiKeyConfigured: true,
  enabled: true,
  disabledProfiles: [],
  contextPolicy: { budgetChars: 4_000, tiers: ['L0', 'L1'] },
  contextPolicies: {},
  degraded: { degraded: false, lastError: null, lastFailedAt: null, lastOkAt: FIXED_NOW },
  tailscaleIPv4: '100.64.0.42'
} satisfies GatewaySettingsSummary;

const MOCK_SKILLS = [{
  id: 'gah-manager',
  version: '1.0.0',
  displayName: 'GAH manager',
  description: 'Coordinates mock GAH work without touching a provider.',
  backends: ['codex', 'claude'],
  source: 'mock/in-memory',
  bound: true
}] satisfies SkillSummary[];

const MOCK_ADMIN_PENDING = {
  current: { hash: '1111111111111111111111111111111111111111', short: '1111111', subject: 'Current mock build' },
  latest: { hash: '2222222222222222222222222222222222222222', short: '2222222', subject: 'Pending mock update' },
  commitsBehind: 1,
  upToDate: false
} satisfies AdminUpdatePendingInfo;

const MOCK_ADMIN_IDLE = {
  status: 'idle',
  startedAt: null,
  finishedAt: null,
  exitCode: null,
  pid: null,
  output: ''
} satisfies AdminUpdateState;

const MOCK_USAGE_ROLLUP = {
  profile: 'fixture',
  since: FIXED_NOW - 7 * 24 * 60 * 60 * 1_000,
  generated_at: FIXED_NOW,
  rows: [{
    backend: 'codex',
    model: 'gpt-5.3-codex',
    day: '2023-11-14',
    turns: 3,
    input_tokens: 1_200,
    output_tokens: 800,
    total_tokens: 2_000,
    estimated_cost_usd: 0.04
  }],
  unattributed_turns: 0,
  tickets: [{
    ticket: '#1087',
    title: 'Mock control plane',
    turns: 3,
    total_tokens: 2_000,
    estimated_cost_usd: 0.04,
    backends: { codex: 2_000 }
  }]
} satisfies UsageRollupSummary;

const MOCK_LEDGER_ENTRY = {
  timestamp: new Date(FIXED_NOW).toISOString(),
  session_id: 'mock-session-1',
  profile: 'fixture',
  display_name: 'Fixture',
  repo_id: 'git-agent-harness',
  repo: 'Kh1ng/git-agent-harness',
  local_path: '/mock/in-memory-only',
  provider: 'github',
  backend: 'codex',
  requested_backend: 'codex',
  effective_backend: 'codex',
  requested_model: 'gpt-5.3-codex',
  effective_model: 'gpt-5.3-codex',
  routing_reason: 'mock fixture',
  fallback_used: false,
  confidence_impact: null,
  human_required: false,
  mode: 'implement',
  target_summary: 'Mock route coverage',
  work_id: 'mock-work-1',
  source_issue_number: '1087',
  work_title: 'Mock control plane',
  branch: 'feat/mock-control-plane-1087',
  session_dir: '/mock/in-memory-only',
  duration_seconds: 42,
  backend_exit_code: 0,
  validation_result: 'pass',
  commit_attempted: true,
  commit_created: true,
  push_attempted: false,
  push_succeeded: false,
  mr_attempted: false,
  mr_created: false,
  mr_url: null,
  files_changed: 2,
  insertions: 20,
  deletions: 3,
  error_summary: null,
  usage: {
    usage_source: 'mock',
    input_tokens: 1_200,
    output_tokens: 800,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    total_tokens: 2_000,
    requests_count: 1,
    estimated_cost_usd: 0.04,
    actual_cost_usd: null,
    quota_window: null,
    quota_used_percent: null,
    quota_remaining_percent: null,
    quota_reset_at: null
  }
} satisfies LedgerEntry;

const MOCK_PRS = [
  {
    number: 12,
    title: 'Ship the PR chat mode',
    url: 'https://github.com/fixture/repo/pull/12',
    author: 'octocat',
    headRefName: 'feat/pr-chat',
    isDraft: false,
    reviewState: 'APPROVED',
    updatedAt: '2026-08-28T12:00:00Z'
  },
  {
    number: 11,
    title: 'Draft the changelog',
    url: 'https://github.com/fixture/repo/pull/11',
    author: 'hubot',
    headRefName: 'docs/changelog',
    isDraft: true,
    reviewState: 'REVIEW_REQUIRED',
    updatedAt: '2026-08-27T12:00:00Z'
  }
] satisfies ChatPrSummary[];

const MOCK_ISSUES = [{
  number: 1087,
  title: 'Mock control plane: cover Settings and non-chat REST surfaces',
  url: 'https://github.com/fixture/repo/issues/1087',
  labels: ['enhancement', 'area:testing'],
  updatedAt: '2026-08-31T05:56:32Z'
}] satisfies ChatIssueSummary[];

const MOCK_PROFILE_CONFIG = {
  profile: 'fixture',
  delivery_mode: 'pr',
  merge_policy: 'auto',
  max_fix_attempts_per_mr: 2,
  max_implementation_failures_per_ticket: 8,
  max_review_cycles_per_ticket: 3,
  max_paid_reviews_per_ticket: 3,
  backend_instances: [],
  pm_candidates: [],
  improve_candidates: [],
  review_candidates: [],
  task_routing_rules: [],
  routine_reviewer: null,
  escalatory_reviewers: [],
  context: {
    global: {
      enabled: true,
      soft_limit_tokens: 80_000,
      hard_limit_tokens: 150_000,
      compact_after_tool_calls: 20,
      fresh_context_on_review: true,
      fresh_context_on_fix: true,
      include_full_git_history: false,
      include_full_worker_transcript_in_review: false,
      recent_history_tokens: 20_000
    },
    profile_override: null,
    effective_by_backend: []
  },
  notifications: {
    configured: false,
    transport: null,
    manager_wake_autonomy: 'off',
    env_file_configured: false,
    env_file_prod_configured: false
  }
} satisfies SettingsConfigProfileSummary;

function sessionKey(profile: string, sessionId: string): string {
  return `${profile}#${sessionId}`;
}

function conversationKey(profile: string, sessionId?: string): string {
  return sessionKey(profile, sessionId ?? 'default');
}

function freshConversation(): ConversationState {
  return {
    turns: [],
    cursor: 0,
    streaming: null,
    permission: null,
    requestId: null,
    steers: [],
    reconnectResumed: false
  };
}

function seededSession(): ChatSessionSummary {
  return {
    id: 'mock-session-1',
    profile: 'fixture',
    worktreePath: '/mock/in-memory-only',
    branch: 'gah/mock-session-1',
    backend: 'codex',
    model: 'gpt-5.3-codex',
    reasoningEffort: null,
    title: 'Mock session',
    createdAt: FIXED_NOW,
    lastActiveAt: FIXED_NOW,
    archivedAt: null,
    outcome: 'live',
    settledAt: null,
    settledReason: null
  } satisfies ChatSessionSummary;
}

/** A second project plus settled/archived sessions, so the session picker's
 * cross-project groups and its Archive section have data in every scenario.
 * 'fixture-two' is deliberately absent from /api/profiles: the picker must
 * fall back to the raw profile id for its group name. */
function seededCrossProjectSessions(): ChatSessionSummary[] {
  return [
    {
      id: 'mock-settled-1',
      profile: 'fixture',
      worktreePath: null,
      branch: 'gah/mock-settled-1',
      backend: 'codex',
      model: null,
      reasoningEffort: null,
      title: 'Settled mock session',
      createdAt: FIXED_NOW - 4_000,
      lastActiveAt: FIXED_NOW - 3_000,
      archivedAt: FIXED_NOW - 3_000,
      outcome: 'settled',
      settledAt: FIXED_NOW - 3_000,
      settledReason: 'merged'
    } satisfies ChatSessionSummary,
    {
      id: 'mock-second-1',
      profile: 'fixture-two',
      worktreePath: '/mock/in-memory-only',
      branch: 'gah/mock-second-1',
      backend: 'codex',
      model: null,
      reasoningEffort: null,
      title: 'Second project session',
      createdAt: FIXED_NOW - 2_000,
      lastActiveAt: FIXED_NOW - 1_000,
      archivedAt: null,
      outcome: 'live',
      settledAt: null,
      settledReason: null
    } satisfies ChatSessionSummary,
    {
      id: 'mock-second-2',
      profile: 'fixture-two',
      worktreePath: null,
      branch: 'gah/mock-second-2',
      backend: 'codex',
      model: null,
      reasoningEffort: null,
      title: 'Archived second session',
      createdAt: FIXED_NOW - 6_000,
      lastActiveAt: FIXED_NOW - 5_000,
      archivedAt: FIXED_NOW - 5_000,
      outcome: 'archived',
      settledAt: null,
      settledReason: null
    } satisfies ChatSessionSummary
  ];
}

function defaultBackendFor(scenario: MockScenarioName): string {
  return scenario === 'models-agy' ? 'agy' : scenario.startsWith('models-') ? 'codex' : 'codex';
}

function createState(scenario: MockScenarioName, reset: number, previewOrigin?: string): MockState {
  const session = seededSession();
  const previews = new Map<string, ChatPreviewInfo>();  if (scenario === 'preview-available') {
    previews.set(sessionKey(session.profile, session.id), {
      profile: session.profile,
      sessionId: session.id,
      devPort: 4173,
      listenPort: 0,
      url: `${previewOrigin ?? 'http://127.0.0.1:3774'}/mock-preview/available`
    } satisfies ChatPreviewInfo);
  }
  if (scenario === 'preview-blocked') {
    previews.set(sessionKey(session.profile, session.id), {
      profile: session.profile,
      sessionId: session.id,
      devPort: 4173,
      listenPort: 443,
      url: 'https://blocked.mock.invalid:443/preview'
    } satisfies ChatPreviewInfo);
  }

  return {
    scenario,
    reset,
    connectionCount: 0,
    settings: {
      defaultBackend: defaultBackendFor(scenario),
      profileOverrides: {},
      availableBackends: BACKENDS
    } satisfies ManagerChatSettingsSummary,
    profiles: structuredClone(MOCK_PROFILES),
    config: { current_manager: null },
    gateway: structuredClone(MOCK_GATEWAY),
    skills: structuredClone(MOCK_SKILLS),
    loopRunning: true,
    adminUpdate: structuredClone(MOCK_ADMIN_IDLE),
    gitPrs: structuredClone(MOCK_PRS),
    selectedModels: { codex: 'gpt-5.3-codex', claude: 'claude-sonnet-4-5', opencode: 'openai/gpt-5.2', agy: null },
    selectedEfforts: { codex: 'medium', claude: 'standard', opencode: 'high', agy: null },
    sessions: new Map([
      [session.id, session],
      ...seededCrossProjectSessions().map((seeded) => [seeded.id, seeded] as const)
    ]),
    conversations: new Map(),
    previews,
    timers: new Set()
  };
}

function modelSummary(state: MockState, backend: string): ManagerModelsSummary {
  if (state.scenario === 'models-empty' || state.scenario === 'models-agy' || backend === 'agy' || backend === 'vibe' || backend === 'hermes') {
    return {
      models: [],
      currentModelId: null,
      reasoningEfforts: [],
      currentReasoningEffortId: null
    } satisfies ManagerModelsSummary;
  }

  if (backend === 'claude') {
    return {
      models: [
        { id: 'claude-sonnet-4-5', name: 'Claude Sonnet 4.5' },
        { id: 'claude-opus-4-1', name: 'Claude Opus 4.1' }
      ],
      currentModelId: state.selectedModels.claude ?? 'claude-sonnet-4-5',
      reasoningEfforts: [
        { id: 'standard', name: 'Standard' },
        { id: 'extended', name: 'Extended' }
      ],
      currentReasoningEffortId: state.selectedEfforts.claude ?? 'standard'
    } satisfies ManagerModelsSummary;
  }

  if (backend === 'opencode') {
    return {
      models: [
        { id: 'openai/gpt-5.2', name: 'OpenAI GPT-5.2' },
        { id: 'anthropic/claude-sonnet-4-5', name: 'Anthropic Claude Sonnet 4.5' }
      ],
      currentModelId: state.selectedModels.opencode ?? 'openai/gpt-5.2',
      reasoningEfforts: [{ id: 'high', name: 'High' }],
      currentReasoningEffortId: state.selectedEfforts.opencode ?? 'high'
    } satisfies ManagerModelsSummary;
  }

  return {
    models: [
      { id: 'gpt-5.3-codex', name: 'GPT-5.3 Codex', description: 'Mock default' },
      { id: 'gpt-5.3-codex-spark', name: 'GPT-5.3 Codex Spark' }
    ],
    currentModelId: state.selectedModels.codex ?? 'gpt-5.3-codex',
    reasoningEfforts: [
      { id: 'low', name: 'Low' },
      { id: 'medium', name: 'Medium' },
      { id: 'xhigh', name: 'Extra high' }
    ],
    currentReasoningEffortId: state.selectedEfforts.codex ?? 'medium'
  } satisfies ManagerModelsSummary;
}

function jsonError(res: Response, status: number, error: string, message: string): void {
  res.status(status).json({ error, message });
}

function bodyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function wait(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

export function createMockControlPlane(options: MockControlPlaneOptions = {}) {
  const app = express();
  const server = createServer(app);
  const wss = new WebSocketServer({ noServer: true });
  const clients = new Set<WebSocket>();
  let previewOrigin = options.previewOrigin;
  let state = createState(options.scenario ?? 'normal', 1, previewOrigin);

  app.use(cors());
  app.use(express.json());

  function schedule(callback: () => void, delayMs: number): void {
    const timer = setTimeout(() => {
      state.timers.delete(timer);
      callback();
    }, delayMs);
    state.timers.add(timer);
  }

  function clearTimers(): void {
    for (const timer of state.timers) clearTimeout(timer);
    state.timers.clear();
  }

  function reset(nextScenario: MockScenarioName = state.scenario, disconnect = true): void {
    const resetCount = state.reset + 1;
    clearTimers();
    state = createState(nextScenario, resetCount, previewOrigin);
    if (disconnect) {
      schedule(() => {
        for (const client of clients) client.close(1012, 'mock scenario reset');
      }, 10);
    }
  }

  function getConversation(profile: string, sessionId?: string): ConversationState {
    const key = conversationKey(profile, sessionId);
    let conversation = state.conversations.get(key);
    if (!conversation) {
      conversation = freshConversation();
      state.conversations.set(key, conversation);
    }
    return conversation;
  }

  function send(ws: WebSocket, message: ServerMessage): void {
    if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(message));
  }

  function broadcast(message: ServerMessage): void {
    for (const client of clients) send(client, message);
  }

  function sendUpdated(profile: string, requestId: string, sessionId?: string): void {
    broadcast({
      type: 'manager.chat.updated',
      profile,
      requestId,
      ...(sessionId ? { sessionId } : {})
    } satisfies ServerMessage);
  }

  function finishTurn(ws: WebSocket | null, profile: string, sessionId: string | undefined, cancelled: boolean): void {
    const conversation = getConversation(profile, sessionId);
    const requestId = conversation.requestId ?? 'mock-turn';
    const partial = conversation.streaming?.partialText ?? '';
    const reply = cancelled
      ? partial || 'partial response'
      : state.scenario === 'reconnect-stream'
        ? partial
        : 'Mock turn complete after multiple chunks.';
    conversation.streaming = null;
    conversation.permission = null;
    conversation.cursor += 2;
    conversation.turns.push({
      role: 'assistant',
      text: reply,
      backend: state.settings.profileOverrides[profile] ?? state.settings.defaultBackend,
      model: state.selectedModels.codex ?? null,
      usage: null,
      timestamp: FIXED_NOW + conversation.cursor
    } satisfies ChatTranscriptTurn);
    if (cancelled) {
      conversation.turns.push({ role: 'system', text: '[cancelled]', timestamp: FIXED_NOW + conversation.cursor + 1 } satisfies ChatTranscriptTurn);
    }
    if (ws) {
      send(ws, {
        type: 'manager.chat.reply',
        requestId,
        profile,
        ...(sessionId ? { sessionId } : {}),
        reply,
        backend: state.settings.profileOverrides[profile] ?? state.settings.defaultBackend,
        model: state.selectedModels.codex ?? null,
        usage: null,
        cancelled: cancelled || undefined
      } satisfies ServerMessage);
    }
    sendUpdated(profile, requestId, sessionId);
  }

  function startNormalTurn(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.send' }>): void {
    const conversation = getConversation(message.profile, message.sessionId);
    conversation.requestId = message.requestId;
    conversation.cursor += 2;
    conversation.turns.push({ role: 'user', text: message.message, timestamp: FIXED_NOW + conversation.cursor } satisfies ChatTranscriptTurn);
    conversation.streaming = { turn: 1, partialText: '', backend: 'codex', model: 'gpt-5.3-codex' };

    schedule(() => {
      conversation.streaming!.partialText += 'Mock turn ';
      conversation.cursor += 1;
      broadcast({
        type: 'manager.chat.chunk', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, seq: conversation.cursor, text: 'Mock turn '
      } satisfies ServerMessage);
    }, 80);
    schedule(() => {
      conversation.cursor += 1;
      broadcast({
        type: 'manager.chat.toolCall', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, toolCallId: 'mock-tool-1', name: 'read', title: 'Read fixture contracts',
        kind: 'read', status: 'pending', locations: ['/mock/contracts/ws.ts'], summary: null
      } satisfies ServerMessage);
    }, 140);
    schedule(() => {
      conversation.streaming!.partialText += 'complete after multiple chunks.';
      conversation.cursor += 1;
      broadcast({
        type: 'manager.chat.chunk', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, seq: conversation.cursor, text: 'complete after multiple chunks.'
      } satisfies ServerMessage);
    }, 220);
    schedule(() => {
      conversation.cursor += 1;
      const tool = {
        toolCallId: 'mock-tool-1', name: 'read', title: 'Read fixture contracts', kind: 'read' as const,
        status: 'completed' as const, locations: ['/mock/contracts/ws.ts'], summary: 'Typed fixture inspected.'
      };
      conversation.turns.push({ role: 'tool', text: tool.summary, tool, timestamp: FIXED_NOW + conversation.cursor } satisfies ChatTranscriptTurn);
      broadcast({
        type: 'manager.chat.toolCall', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, ...tool
      } satisfies ServerMessage);
    }, 300);
    schedule(() => finishTurn(ws, message.profile, message.sessionId, false), 380);
  }

  function startSlowTurn(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.send' }>): void {
    const conversation = getConversation(message.profile, message.sessionId);
    conversation.requestId = message.requestId;
    conversation.cursor += 2;
    conversation.turns.push({ role: 'user', text: message.message, timestamp: FIXED_NOW + conversation.cursor } satisfies ChatTranscriptTurn);
    conversation.streaming = { turn: 1, partialText: 'Working slowly…', backend: 'codex', model: 'gpt-5.3-codex' };
    send(ws, {
      type: 'manager.chat.chunk', requestId: message.requestId, profile: message.profile,
      ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      turn: 1, seq: ++conversation.cursor, text: 'Working slowly…'
    } satisfies ServerMessage);
  }

  function startReconnectTurn(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.send' }>, permission: boolean): void {
    const conversation = getConversation(message.profile, message.sessionId);
    conversation.requestId = message.requestId;
    conversation.cursor += 2;
    conversation.turns.push({ role: 'user', text: message.message, timestamp: FIXED_NOW + conversation.cursor } satisfies ChatTranscriptTurn);
    conversation.streaming = { turn: 1, partialText: permission ? 'Waiting for approval…' : 'Before disconnect…', backend: 'codex', model: 'gpt-5.3-codex' };
    if (permission) {
      conversation.permission = {
        turn: 1,
        permissionId: 'mock-permission-1',
        title: 'Run focused Playwright tests',
        options: [
          { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
          { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
        ],
        locations: ['/mock/package.json']
      };
      send(ws, {
        type: 'manager.chat.permission', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, permissionId: conversation.permission.permissionId, title: conversation.permission.title,
        options: conversation.permission.options, locations: conversation.permission.locations
      } satisfies ServerMessage);
    } else {
      send(ws, {
        type: 'manager.chat.chunk', requestId: message.requestId, profile: message.profile,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        turn: 1, seq: ++conversation.cursor, text: 'Before disconnect…'
      } satisfies ServerMessage);
    }
    schedule(() => ws.close(1012, 'deterministic mock disconnect'), 120);
  }

  function resumeReconnectTurn(profile: string, sessionId: string | undefined): void {
    const conversation = getConversation(profile, sessionId);
    if (conversation.reconnectResumed || !conversation.streaming || state.scenario !== 'reconnect-stream') return;
    conversation.reconnectResumed = true;
    schedule(() => {
      conversation.streaming!.partialText += ' resumed after reconnect.';
      broadcast({
        type: 'manager.chat.chunk', requestId: conversation.requestId ?? 'mock-turn', profile,
        ...(sessionId ? { sessionId } : {}), turn: 1, seq: ++conversation.cursor, text: ' resumed after reconnect.'
      } satisfies ServerMessage);
    }, 100);
    schedule(() => finishTurn(null, profile, sessionId, false), 220);
  }

  function handleClientMessage(ws: WebSocket, message: ClientMessage): void {
    switch (message.type) {
      case 'client.hello':
        return;
      case 'ping':
        send(ws, { type: 'server.ping', timestamp: message.timestamp } satisfies ServerMessage);
        return;
      case 'manager.chat.historyRequest': {
        const conversation = getConversation(message.profile, message.sessionId);
        send(ws, {
          type: 'manager.chat.history',
          requestId: message.requestId,
          profile: message.profile,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          turns: conversation.turns,
          cursor: conversation.cursor,
          streaming: conversation.streaming,
          permission: conversation.permission
        } satisfies ServerMessage);
        if (state.connectionCount > 1) resumeReconnectTurn(message.profile, message.sessionId);
        return;
      }
      case 'manager.chat.send':
        if (state.scenario === 'ws-error') {
          send(ws, { type: 'error', requestId: message.requestId, error: 'Mock WebSocket turn failure' } satisfies ServerMessage);
        } else if (state.scenario === 'slow-cancel-steer') {
          startSlowTurn(ws, message);
        } else if (state.scenario === 'reconnect-stream') {
          startReconnectTurn(ws, message, false);
        } else if (state.scenario === 'reconnect-permission') {
          startReconnectTurn(ws, message, true);
        } else {
          startNormalTurn(ws, message);
        }
        return;
      case 'manager.chat.steer': {
        const conversation = getConversation(message.profile, message.sessionId);
        if (!conversation.streaming || message.message.includes('reject')) {
          send(ws, { type: 'error', requestId: message.requestId, error: 'Mock backend rejected steering' } satisfies ServerMessage);
          return;
        }
        conversation.steers.push(message.message);
        conversation.turns.push({ role: 'user', text: message.message, timestamp: FIXED_NOW + ++conversation.cursor } satisfies ChatTranscriptTurn);
        send(ws, {
          type: 'manager.chat.steered', requestId: message.requestId, profile: message.profile,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}), outcome: 'injected'
        } satisfies ServerMessage);
        return;
      }
      case 'manager.chat.cancel':
        if (getConversation(message.profile, message.sessionId).streaming) finishTurn(ws, message.profile, message.sessionId, true);
        else send(ws, { type: 'error', requestId: message.requestId, error: 'No turn is in flight for this profile.' } satisfies ServerMessage);
        return;
      case 'manager.chat.permission.respond': {
        const conversation = getConversation(message.profile, message.sessionId);
        if (!conversation.permission || conversation.permission.permissionId !== message.permissionId) {
          send(ws, { type: 'error', requestId: message.requestId, error: 'No pending permission request for this conversation.' } satisfies ServerMessage);
          return;
        }
        conversation.permission = null;
        conversation.streaming = null;
        conversation.turns.push({
          role: 'assistant', text: `Permission ${message.optionId} received after reconnect.`, backend: 'codex',
          model: 'gpt-5.3-codex', usage: null, timestamp: FIXED_NOW + ++conversation.cursor
        } satisfies ChatTranscriptTurn);
        sendUpdated(message.profile, conversation.requestId ?? message.requestId, message.sessionId);
        return;
      }
      case 'manager.chat.sessionList':
        send(ws, {
          type: 'manager.chat.sessionList', requestId: message.requestId, profile: message.profile,
          sessions: [...state.sessions.values()].filter((session) => session.profile === message.profile)
        } satisfies ServerMessage);
        return;
      case 'manager.chat.sessionCreate': {
        const created = createSession(message.profile, message.backend, message.model ?? null, message.title);
        send(ws, { type: 'manager.chat.sessionCreated', requestId: message.requestId, profile: message.profile, session: created } satisfies ServerMessage);
        return;
      }
      case 'manager.chat.sessionUpdate': {
        const updated = updateSession(message.profile, message.sessionId, message);
        if (updated) send(ws, { type: 'manager.chat.sessionUpdated', requestId: message.requestId, profile: message.profile, session: updated } satisfies ServerMessage);
        else send(ws, { type: 'error', requestId: message.requestId, error: 'Mock session not found' } satisfies ServerMessage);
        return;
      }
      case 'manager.chat.sessionArchive': {
        const archived = archiveSession(message.profile, message.sessionId);
        if (archived) send(ws, { type: 'manager.chat.sessionArchived', requestId: message.requestId, profile: message.profile, session: archived } satisfies ServerMessage);
        else send(ws, { type: 'error', requestId: message.requestId, error: 'Mock archive failed' } satisfies ServerMessage);
        return;
      }
      default:
        send(ws, { type: 'error', requestId: 'requestId' in message ? message.requestId : 'mock-unknown', error: `Unsupported mock message: ${message.type}` } satisfies ServerMessage);
    }
  }

  function createSession(profile: string, backend?: string, model: string | null = null, title?: string): ChatSessionSummary {
    const id = `mock-session-${state.sessions.size + 1}`;
    const created = {
      id,
      profile,
      worktreePath: '/mock/in-memory-only',
      branch: `gah/${id}`,
      backend: backend ?? state.settings.defaultBackend,
      model,
      reasoningEffort: null,
      title: title ?? null,
      createdAt: FIXED_NOW + state.sessions.size,
      lastActiveAt: FIXED_NOW + state.sessions.size,
      archivedAt: null,
      outcome: 'live',
      settledAt: null,
      settledReason: null
    } satisfies ChatSessionSummary;
    state.sessions.set(id, created);
    return created;
  }

  function updateSession(profile: string, id: string, patch: { backend?: string; model?: string | null; reasoningEffort?: string | null; title?: string }): ChatSessionSummary | null {
    const current = state.sessions.get(id);
    if (!current || current.profile !== profile || current.archivedAt !== null) return null;
    const updated = {
      ...current,
      ...(patch.backend !== undefined ? { backend: patch.backend } : {}),
      ...(patch.model !== undefined ? { model: patch.model } : {}),
      ...(patch.reasoningEffort !== undefined ? { reasoningEffort: patch.reasoningEffort } : {}),
      ...(patch.title !== undefined ? { title: patch.title } : {}),
      lastActiveAt: FIXED_NOW + state.reset
    } satisfies ChatSessionSummary;
    state.sessions.set(id, updated);
    return updated;
  }

  function archiveSession(profile: string, id: string): ChatSessionSummary | null {
    if (state.scenario === 'archive-failure') return null;
    const current = state.sessions.get(id);
    if (!current || current.profile !== profile) return null;
    const archived = {
      ...current,
      worktreePath: null,
      archivedAt: FIXED_NOW + state.reset,
      outcome: 'archived',
      settledAt: null,
      settledReason: null
    } satisfies ChatSessionSummary;
    state.sessions.set(id, archived);
    state.previews.delete(sessionKey(profile, id));
    return archived;
  }

  /** PR → chat: a read-only session seeded with the PR -- worktree-less,
   * riding the PR's head branch, with the PR as the opening message. */
  function createPrSession(profile: string, backend: string | undefined, model: string | null, pr: ChatPrSummary): ChatSessionSummary {
    const id = `mock-session-${state.sessions.size + 1}`;
    const created = {
      id,
      profile,
      worktreePath: null,
      branch: pr.headRefName ?? `gah/pr/${id}`,
      backend: backend ?? state.settings.defaultBackend,
      model,
      reasoningEffort: null,
      title: `#${pr.number} ${pr.title}`,
      createdAt: FIXED_NOW + state.sessions.size,
      lastActiveAt: FIXED_NOW + state.sessions.size,
      archivedAt: null,
      outcome: 'live',
      settledAt: null,
      settledReason: null
    } satisfies ChatSessionSummary;
    state.sessions.set(id, created);
    const conversation = getConversation(profile, id);
    conversation.cursor += 2;
    conversation.turns.push({
      role: 'user',
      text: `#${pr.number} ${pr.title}\n\nHead branch: ${pr.headRefName}\n(${pr.url})`,
      timestamp: FIXED_NOW + conversation.cursor
    } satisfies ChatTranscriptTurn);
    return created;
  }

  app.get('/health', (_req, res) => res.json({ status: 'ok', mode: 'mock', scenario: state.scenario }));

  app.get('/api/mock/scenarios', (_req, res) => {
    res.json({
      active: state.scenario,
      scenarios: Object.entries(MOCK_SCENARIOS).map(([name, metadata]) => ({ name, ...metadata }))
    });
  });
  app.get('/api/mock/state', (_req, res) => {
    res.json({
      scenario: state.scenario,
      reset: state.reset,
      connections: state.connectionCount,
      profiles: state.profiles,
      config: state.config,
      gateway: state.gateway,
      skills: state.skills,
      loopRunning: state.loopRunning,
      adminUpdate: state.adminUpdate,
      gitPrs: state.gitPrs,
      sessions: [...state.sessions.values()],
      previews: [...state.previews.values()]
    });
  });
  app.post('/api/mock/scenario', (req, res) => {
    const requested = req.body?.name ?? req.query.name;
    if (!isMockScenarioName(requested)) {
      jsonError(res, 400, 'Unknown mock scenario', `Choose one of: ${Object.keys(MOCK_SCENARIOS).join(', ')}`);
      return;
    }
    reset(requested);
    res.json({ active: state.scenario, reset: state.reset });
  });
  app.post('/api/mock/reset', (_req, res) => {
    reset();
    res.json({ active: state.scenario, reset: state.reset });
  });

  app.get('/mock-preview/available', (_req, res) => {
    res.type('html').send('<!doctype html><title>Mock preview</title><main><h1>Mock preview available</h1><p>No dev server or worktree was created.</p></main>');
  });

  // One scenario covers every frontend write without teaching each route a
  // second failure branch. Mock-control routes stay usable because they are
  // registered above this middleware.
  app.use('/api', (req, res, next) => {
    if (state.scenario === 'rest-error' && req.method !== 'GET') {
      jsonError(res, 503, 'Mock REST failure', `Mock ${req.method} ${req.path} failed`);
      return;
    }
    next();
  });

  app.get('/api/profiles', (_req, res) => res.json(state.profiles));
  app.post('/api/profiles', (req, res) => {
    const required = ['name', 'display_name', 'repo_id', 'provider', 'repo', 'local_path', 'artifact_root'] as const;
    const missing = required.filter((field) => !bodyString(req.body?.[field]));
    if (missing.length > 0) return jsonError(res, 400, 'Invalid profile', `Missing required field(s): ${missing.join(', ')}`);
    const name = req.body.name as string;
    if (state.profiles.some((profile) => profile.name === name)) {
      return jsonError(res, 409, 'Invalid profile', `Profile '${name}' already exists`);
    }
    state.profiles.push({
      name,
      display_name: req.body.display_name as string,
      repo_id: req.body.repo_id as string,
      provider: req.body.provider as string,
      repo: req.body.repo as string,
      local_path: req.body.local_path as string,
      worktree_base: '/mock/worktrees',
      web_url: req.body.provider === 'github' ? `https://github.com/${req.body.repo as string}` : null,
      max_parallel_workers: typeof req.body.max_parallel_workers === 'number' ? req.body.max_parallel_workers : null,
      max_open_managed_mrs: 1,
      manager_wake_autonomy: req.body.manager_wake_autonomy ?? 'off',
      delivery_mode: 'pr',
      validation_timeout_seconds: typeof req.body.validation_timeout_seconds === 'number' ? req.body.validation_timeout_seconds : 300,
      chat_session_idle_days: 14
    } satisfies ProfileSummary);
    res.status(201).json({ success: true, message: `Profile '${name}' added` });
  });
  app.patch('/api/profiles/:name', (req, res) => {
    const index = state.profiles.findIndex((profile) => profile.name === req.params.name);
    if (index < 0) return jsonError(res, 404, 'Profile not found', req.params.name);
    const current = state.profiles[index];
    const clear = Array.isArray(req.body?.clear) ? req.body.clear : [];
    state.profiles[index] = {
      ...current,
      ...(bodyString(req.body?.display_name) ? { display_name: req.body.display_name as string } : {}),
      ...(bodyString(req.body?.repo_id) ? { repo_id: req.body.repo_id as string } : {}),
      ...(bodyString(req.body?.provider) ? { provider: req.body.provider as string } : {}),
      ...(bodyString(req.body?.repo) ? { repo: req.body.repo as string } : {}),
      ...(bodyString(req.body?.local_path) ? { local_path: req.body.local_path as string } : {}),
      ...(typeof req.body?.max_parallel_workers === 'number' ? { max_parallel_workers: req.body.max_parallel_workers } : {}),
      ...(typeof req.body?.manager_wake_autonomy === 'string' ? { manager_wake_autonomy: req.body.manager_wake_autonomy } : {}),
      ...(typeof req.body?.validation_timeout_seconds === 'number'
        ? { validation_timeout_seconds: req.body.validation_timeout_seconds }
        : clear.includes('validation_timeout_seconds') ? { validation_timeout_seconds: 300 } : {})
    } satisfies ProfileSummary;
    res.json({ success: true, message: `Profile '${req.params.name}' updated` });
  });
  app.delete('/api/profiles/:name', (req, res) => {
    const index = state.profiles.findIndex((profile) => profile.name === req.params.name);
    if (index < 0) return jsonError(res, 404, 'Profile not found', req.params.name);
    state.profiles.splice(index, 1);
    res.json({ success: true, message: `Profile '${req.params.name}' removed` });
  });

  app.get('/api/projects', (_req, res) => res.json(state.profiles));
  app.post('/api/projects/import', (req, res) => {
    const gitUrl = bodyString(req.body?.gitUrl)?.trim();
    if (!gitUrl) return jsonError(res, 400, 'Invalid project import', 'gitUrl is required');
    let parsed: URL;
    try {
      parsed = new URL(gitUrl);
    } catch {
      return jsonError(res, 400, 'Invalid project import', 'gitUrl must be an absolute URL');
    }
    const parts = parsed.pathname.replace(/\.git$/, '').split('/').filter(Boolean);
    if (parts.length < 2) return jsonError(res, 400, 'Invalid project import', 'gitUrl must identify an owner and repository');
    const repo = parts.slice(-2).join('/');
    const baseName = parts.at(-1)!;
    const name = state.profiles.some((profile) => profile.name === baseName)
      ? `${baseName}-${state.profiles.length + 1}`
      : baseName;
    const project = {
      ...structuredClone(MOCK_PROFILES[0]),
      name,
      display_name: baseName,
      repo_id: baseName,
      repo,
      local_path: `/mock/imports/${name}`,
      web_url: gitUrl.replace(/\.git$/, '')
    } satisfies ProfileSummary;
    state.profiles.push(project);
    res.status(201).json({
      project,
      checkoutPath: project.local_path,
      checkoutStatus: 'cloned',
      detectedLanguages: ['TypeScript'],
      validationCommands: ['npm test']
    } satisfies ProjectImportResult);
  });

  app.get('/api/status', (_req, res) => res.json(STATUS_FIXTURE));
  app.get('/api/quota', (_req, res) => res.json(QUOTA_FIXTURE));
  app.get('/api/usage/rollup', (req, res) => {
    res.json({ ...MOCK_USAGE_ROLLUP, profile: bodyString(req.query.profile) ?? 'fixture' } satisfies UsageRollupSummary);
  });
  app.get('/api/report', (_req, res) => res.json(REPORT_FIXTURE));
  app.get('/api/report/series', (_req, res) => {
    res.json({
      ledger_path: REPORT_FIXTURE.ledger_path,
      since: REPORT_FIXTURE.since,
      bucket: 'daily',
      profile: REPORT_FIXTURE.profile,
      series: []
    } satisfies ReportSeriesData);
  });
  app.get('/api/events', (_req, res) => res.json([]));
  app.get('/api/controller-activity', (_req, res) => res.json([]));
  app.get('/api/work/:workId', (req, res) => {
    res.json([{ ...MOCK_LEDGER_ENTRY, work_id: req.params.workId }] satisfies LedgerEntry[]);
  });
  app.get('/api/doctor', (_req, res) => {
    res.json({
      schema_version: 1,
      generated_at: new Date(FIXED_NOW).toISOString(),
      overall_status: 'ok',
      checks: [{ name: 'mock isolation', status: 'ok', detail: 'in-memory control plane' }]
    } satisfies DoctorSnapshot);
  });
  app.get('/api/config', (_req, res) => res.json(state.config));
  app.post('/api/config', (req, res) => {
    if (Array.isArray(req.body?.clear) && req.body.clear.includes('current_manager')) {
      state.config.current_manager = null;
    } else if (typeof req.body?.current_manager === 'string' || req.body?.current_manager === null) {
      state.config.current_manager = req.body.current_manager;
    }
    res.json({ success: true, message: 'Global config updated' });
  });
  app.get('/api/config/effective', (req, res) => {
    res.json({ ...MOCK_PROFILE_CONFIG, profile: bodyString(req.query.profile) ?? 'fixture' } satisfies SettingsConfigProfileSummary);
  });
  app.get('/api/loop/status', (_req, res) => res.json({ running: state.loopRunning, ...(state.loopRunning ? { pid: 998 } : {}) }));
  app.post('/api/loop/start', (_req, res) => {
    if (state.loopRunning) return res.status(409).json({ started: false, alreadyRunning: true, pid: 998 });
    state.loopRunning = true;
    res.json({ started: true, pid: 998 });
  });
  app.post('/api/loop/stop', (_req, res) => {
    if (!state.loopRunning) return res.status(409).json({ stopped: false, error: 'Mock loop is not running' });
    state.loopRunning = false;
    res.json({ stopped: true });
  });

  app.get('/api/settings/gateway', (_req, res) => res.json(state.gateway));
  app.post('/api/settings/gateway/bootstrap-command', (_req, res) => {
    res.set('Cache-Control', 'no-store');
    res.json({ command: 'GAH_GATEWAY_MODE=remote GAH_GATEWAY_API_KEY=mock-only-token bash bootstrap.sh' } satisfies GatewayBootstrapCommand);
  });
  app.put('/api/settings/gateway', (req, res) => {
    state.gateway = {
      ...state.gateway,
      ...('url' in req.body ? { url: req.body.url ?? 'http://127.0.0.1:8420' } : {}),
      ...(typeof req.body?.enabled === 'boolean' ? { enabled: req.body.enabled } : {}),
      ...(Array.isArray(req.body?.disabledProfiles) ? { disabledProfiles: req.body.disabledProfiles } : {}),
      ...(req.body?.contextPolicy && typeof req.body.contextPolicy === 'object' ? { contextPolicy: req.body.contextPolicy } : {}),
      ...(req.body?.contextPolicies && typeof req.body.contextPolicies === 'object' ? { contextPolicies: req.body.contextPolicies } : {}),
      ...('apiKey' in req.body ? { apiKeyConfigured: typeof req.body.apiKey === 'string' && req.body.apiKey.length > 0 } : {})
    } satisfies GatewaySettingsSummary;
    res.json(state.gateway);
  });
  app.get('/api/skills', (_req, res) => res.json({ skills: state.skills }));

  app.get('/api/git/status', (_req, res) => {
    res.json({ branch: 'feat/mock-control-plane-1087', changes: [{ status: 'M', path: 'apps/server/mock/controlPlane.ts' }], cwd: '/mock/in-memory-only' });
  });
  app.get('/api/git/log', (_req, res) => {
    res.json({ commits: [{ hash: '1111111111111111111111111111111111111111', short: '1111111', subject: 'Mock commit', author: 'GAH', ago: '1 minute ago' }] });
  });
  app.get('/api/git/prs', (_req, res) => res.json({ prs: state.gitPrs }));
  app.post('/api/git/pr', (req, res) => {
    const title = bodyString(req.body?.title);
    if (!title) return jsonError(res, 400, 'Invalid pull request', 'title required');
    const number = Math.max(0, ...state.gitPrs.map((pr) => pr.number)) + 1;
    const url = `https://github.com/fixture/repo/pull/${number}`;
    state.gitPrs.unshift({
      number,
      title,
      url,
      author: 'mock-operator',
      headRefName: 'feat/mock-control-plane-1087',
      isDraft: req.body?.draft === true,
      reviewState: null,
      updatedAt: new Date(FIXED_NOW + number).toISOString()
    });
    res.json({ url });
  });

  app.get('/api/admin/update', (_req, res) => res.json(MOCK_ADMIN_PENDING));
  app.get('/api/admin/update/status', (_req, res) => res.json(state.adminUpdate));
  app.post('/api/admin/update', (_req, res) => {
    if (state.adminUpdate.status === 'running') return res.status(409).json(state.adminUpdate);
    state.adminUpdate = {
      status: 'running',
      startedAt: new Date(FIXED_NOW + state.reset).toISOString(),
      finishedAt: null,
      exitCode: null,
      pid: 999,
      output: 'Mock update started.\n'
    };
    schedule(() => {
      state.adminUpdate = {
        ...state.adminUpdate,
        status: 'success',
        finishedAt: new Date(FIXED_NOW + state.reset + 1_000).toISOString(),
        exitCode: 0,
        pid: null,
        output: `${state.adminUpdate.output}Mock update completed.\n`
      };
    }, 100);
    res.status(202).json(state.adminUpdate);
  });

  app.get('/api/manager-chat/settings', (_req, res) => res.json(state.settings satisfies ManagerChatSettingsSummary));
  app.post('/api/manager-chat/settings', (req, res) => {
    const defaultBackend = bodyString(req.body?.defaultBackend);
    const overrides = req.body?.profileOverrides;
    state.settings = {
      ...state.settings,
      ...(defaultBackend ? { defaultBackend } : {}),
      ...(overrides && typeof overrides === 'object' ? { profileOverrides: overrides as Record<string, string> } : {})
    } satisfies ManagerChatSettingsSummary;
    res.json({ success: true });
  });
  app.get('/api/manager-chat/commands', (_req, res) => {
    const commands = [
      { name: 'compact', description: 'Compact mock context' },
      { name: 'status', description: 'Show mock status' }
    ] satisfies ManagerCommandInfo[];
    res.json({ commands });
  });
  app.get('/api/manager-chat/models', async (req, res) => {
    if (state.scenario === 'models-failure') {
      jsonError(res, 502, 'Failed to load manager chat models', 'Mock model discovery failed');
      return;
    }
    if (state.scenario === 'models-delayed') await wait(1_200);
    const profile = bodyString(req.query.profile) ?? 'fixture';
    const backend = bodyString(req.query.backend) ?? state.settings.profileOverrides[profile] ?? state.settings.defaultBackend;
    res.json(modelSummary(state, backend));
  });
  app.post('/api/manager-chat/model', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const backend = state.settings.profileOverrides[profile] ?? state.settings.defaultBackend;
    const modelId = bodyString(req.body?.modelId);
    if (!modelId) return jsonError(res, 400, 'Missing required field: modelId', 'modelId is required');
    state.selectedModels[backend] = modelId;
    res.json({ success: true });
  });
  app.post('/api/manager-chat/reasoning-effort', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const backend = state.settings.profileOverrides[profile] ?? state.settings.defaultBackend;
    const effortId = bodyString(req.body?.effortId);
    if (!effortId) return jsonError(res, 400, 'Missing required field: effortId', 'effortId is required');
    state.selectedEfforts[backend] = effortId;
    res.json({ success: true });
  });
  app.get('/api/manager-chat/nodes', (_req, res) => {
    const nodes = [{ nodeId: 'mock-central', displayName: 'Mock central', role: 'central', chatCapable: true, lastSeenAt: null }] satisfies ChatNodeInfo[];
    res.json({ nodes });
  });
  app.get('/api/manager-chat/issues', (_req, res) => res.json({ issues: MOCK_ISSUES }));
  app.post('/api/manager-chat/issues/start', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const issueNumber = Number(req.body?.issueNumber);
    if (!Number.isInteger(issueNumber)) {
      return jsonError(res, 400, 'Missing required field: issueNumber', 'issueNumber is required');
    }
    const issue = MOCK_ISSUES.find((candidate) => candidate.number === issueNumber);
    if (!issue) return jsonError(res, 502, 'Failed to start chat from issue', `Issue #${issueNumber} is not open`);
    const created = createSession(
      profile,
      bodyString(req.body?.backend),
      bodyString(req.body?.model) ?? null,
      `#${issue.number} ${issue.title}`
    );
    const session = { ...created, branch: `gah/issue-${issue.number}` } satisfies ChatSessionSummary;
    state.sessions.set(session.id, session);
    const conversation = getConversation(profile, session.id);
    conversation.turns.push({
      role: 'user',
      text: `#${issue.number} ${issue.title}\n\n${issue.url ?? ''}`,
      timestamp: FIXED_NOW + ++conversation.cursor
    } satisfies ChatTranscriptTurn);
    res.status(201).json({ session, existing: false } satisfies ChatIssueStartResult);
  });
  app.get('/api/manager-chat/prs', (_req, res) => res.json({ prs: state.gitPrs }));
  app.post('/api/manager-chat/prs/start', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const prNumber = Number(req.body?.prNumber);
    if (!Number.isInteger(prNumber)) {
      return jsonError(res, 400, 'Missing required field: prNumber', 'prNumber is required');
    }
    const pr = state.gitPrs.find((candidate) => candidate.number === prNumber);
    if (!pr) {
      return jsonError(res, 502, 'Failed to start chat from pull request', `PR #${prNumber} is not open`);
    }
    const created = createPrSession(
      profile,
      bodyString(req.body?.backend),
      bodyString(req.body?.model) ?? null,
      pr
    );
    res.status(201).json({ session: created, existing: false } satisfies ChatPrStartResult);
  });

  app.get('/api/manager-chat/sessions', (req, res) => {
    if (state.scenario === 'rest-error') return jsonError(res, 503, 'Mock REST failure', 'Mock session list unavailable');
    const profile = bodyString(req.query.profile) ?? 'fixture';
    res.json({ sessions: [...state.sessions.values()].filter((session) => session.profile === profile) } satisfies { sessions: ChatSessionSummary[] });
  });
  app.get('/api/manager-chat/sessions/all', (_req, res) => {
    if (state.scenario === 'rest-error') return jsonError(res, 503, 'Mock REST failure', 'Mock session list unavailable');
    const profileNames = new Map(state.profiles.map((profile) => [profile.name, profile.display_name]));
    const byProfile = new Map<string, ChatSessionSummary[]>();
    for (const session of state.sessions.values()) {
      const sessions = byProfile.get(session.profile) ?? [];
      sessions.push(session);
      byProfile.set(session.profile, sessions);
    }
    const projects = [...byProfile.entries()].map(([profile, sessions]) => ({
      profile,
      profileName: profileNames.get(profile) ?? profile,
      sessions: sessions.slice().sort((a, b) => b.lastActiveAt - a.lastActiveAt)
    } satisfies ChatSessionProjectGroup));
    res.json({ projects });
  });
  function storagePayload(profile: string, dryRun: boolean): ChatReclaimResult {
    const sessions = [...state.sessions.values()].filter((session) => session.profile === profile);
    const candidateSessions = sessions.filter((session) => session.outcome === 'live' && session.id === 'mock-session-1');
    return {
      dryRun,
      profiles: [{
        profile,
        idleDays: 14,
        worktreeBytes: sessions.filter((session) => session.outcome === 'live').length * 12_582_912,
        projectedReclaimBytes: candidateSessions.length * 12_582_912,
        sessions: sessions.map((session) => ({
          sessionId: session.id,
          worktreeBytes: session.outcome === 'live' ? 12_582_912 : 0,
          projectedReclaimBytes: candidateSessions.some((candidate) => candidate.id === session.id) ? 12_582_912 : 0,
          idle: candidateSessions.some((candidate) => candidate.id === session.id)
        }))
      }],
      candidates: candidateSessions.map((session) => ({
        profile,
        sessionId: session.id,
        outcome: 'archived',
        reason: 'idle',
        reclaimBytes: 12_582_912
      })),
      sessions: [],
      warnings: []
    };
  }
  app.get('/api/manager-chat/storage', (req, res) => {
    res.json(storagePayload(bodyString(req.query.profile) ?? 'fixture', true));
  });
  app.post('/api/manager-chat/reclaim', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const dryRun = req.body?.dryRun !== false;
    const plan = storagePayload(profile, dryRun);
    if (!dryRun) {
      plan.sessions = plan.candidates
        .map((candidate) => archiveSession(profile, candidate.sessionId))
        .filter((session): session is ChatSessionSummary => session !== null);
    }
    res.json(plan);
  });
  app.post('/api/manager-chat/sessions', (req, res) => {
    const created = createSession(
      bodyString(req.body?.profile) ?? 'fixture',
      bodyString(req.body?.backend),
      bodyString(req.body?.model) ?? null,
      bodyString(req.body?.title)
    );
    res.status(201).json(created satisfies ChatSessionSummary);
  });
  app.post('/api/manager-chat/sessions/update', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const id = bodyString(req.body?.sessionId);
    if (!id) return jsonError(res, 400, 'Missing required field: sessionId', 'sessionId is required');
    const updated = updateSession(profile, id, {
      ...(bodyString(req.body?.backend) ? { backend: req.body.backend as string } : {}),
      ...(typeof req.body?.model === 'string' || req.body?.model === null ? { model: req.body.model as string | null } : {}),
      ...(typeof req.body?.reasoningEffort === 'string' || req.body?.reasoningEffort === null ? { reasoningEffort: req.body.reasoningEffort as string | null } : {}),
      ...(bodyString(req.body?.title) ? { title: req.body.title as string } : {})
    });
    if (!updated) return jsonError(res, 404, 'Mock session not found', id);
    res.json(updated satisfies ChatSessionSummary);
  });
  app.post('/api/manager-chat/sessions/archive', (req, res) => {
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const id = bodyString(req.body?.sessionId);
    const ids: string[] = id
      ? [id]
      : Array.isArray(req.body?.sessionIds)
        ? req.body.sessionIds.filter((value: unknown): value is string => typeof value === 'string')
        : [];
    if (ids.length === 0) return jsonError(res, 400, 'Missing required field: sessionId or sessionIds', 'a session id is required');
    if (state.scenario === 'archive-failure') return jsonError(res, 502, 'Failed to archive chat session', 'Mock archive failed');
    const archived = ids.map((sessionId) => archiveSession(profile, sessionId));
    if (archived.some((session) => session === null)) return jsonError(res, 502, 'Failed to archive chat session', 'Mock archive failed');
    res.json(id ? archived[0] : { sessions: archived });
  });
  app.get('/api/manager-chat/preview', (req, res) => {
    const profile = bodyString(req.query.profile) ?? 'fixture';
    const id = bodyString(req.query.sessionId);
    res.json({ preview: id ? state.previews.get(sessionKey(profile, id)) ?? null : null } satisfies { preview: ChatPreviewInfo | null });
  });
  app.post('/api/manager-chat/preview/set', (req, res) => {
    if (state.scenario === 'preview-error') return jsonError(res, 502, 'Failed to set preview', 'Mock preview target is blocked');
    const profile = bodyString(req.body?.profile) ?? 'fixture';
    const id = bodyString(req.body?.sessionId);
    if (!id) return jsonError(res, 400, 'Missing required field: sessionId', 'sessionId is required');
    if (req.body?.port === null) {
      state.previews.delete(sessionKey(profile, id));
      res.json({ preview: null } satisfies { preview: ChatPreviewInfo | null });
      return;
    }
    const port = Number(req.body?.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return jsonError(res, 400, 'Invalid preview port', 'port must be 1..65535');
    const preview = {
      profile,
      sessionId: id,
      devPort: port,
      listenPort: Number(new URL(previewOrigin ?? 'http://127.0.0.1:3774').port || 80),
      url: `${previewOrigin ?? 'http://127.0.0.1:3774'}/mock-preview/available`
    } satisfies ChatPreviewInfo;
    state.previews.set(sessionKey(profile, id), preview);
    res.json({ preview } satisfies { preview: ChatPreviewInfo | null });
  });

  server.on('upgrade', (request, socket, head) => {
    const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);
    if (url.pathname !== '/ws') {
      socket.destroy();
      return;
    }
    wss.handleUpgrade(request, socket, head, (ws) => wss.emit('connection', ws, request));
  });

  wss.on('connection', (ws) => {
    clients.add(ws);
    state.connectionCount += 1;
    send(ws, {
      type: 'server.welcome',
      serverVersion: '0.0.0-mock',
      serverProviderCatalog: { providers: [] },
      sessions: [],
      providers: {},
      profile: 'fixture',
      mergeRequests: [],
      availability: [],
      blockers: [],
      constraints: [],
      errors: [],
      recentLedger: null,
      backendConfigured: Object.fromEntries(BACKENDS.map((backend) => [backend.id, backend.implemented]))
    } satisfies ServerMessage);
    ws.on('message', (raw) => {
      try {
        handleClientMessage(ws, JSON.parse(raw.toString()) as ClientMessage);
      } catch (error) {
        send(ws, {
          type: 'error',
          requestId: 'mock-parse-error',
          error: `Failed to parse mock WebSocket message: ${error instanceof Error ? error.message : String(error)}`
        } satisfies ServerMessage);
      }
    });
    ws.on('close', () => clients.delete(ws));
  });

  return {
    app,
    server,
    wss,
    get scenario() { return state.scenario; },
    async listen(port = 3774, host = '127.0.0.1'): Promise<RunningMockControlPlane> {
      await new Promise<void>((resolvePromise, reject) => {
        server.once('error', reject);
        server.listen(port, host, () => {
          server.off('error', reject);
          resolvePromise();
        });
      });
      const address = server.address() as AddressInfo;
      previewOrigin = `http://${host}:${address.port}`;
      // Rebuild preview URLs now that an ephemeral port is known in tests.
      if (state.scenario === 'preview-available') reset(state.scenario, false);
      const baseUrl = previewOrigin;
      return {
        baseUrl,
        wsUrl: `ws://${host}:${address.port}/ws`,
        scenario: () => state.scenario,
        close: async () => {
          clearTimers();
          for (const client of clients) client.terminate();
          await new Promise<void>((resolvePromise) => wss.close(() => resolvePromise()));
          await new Promise<void>((resolvePromise) => server.close(() => resolvePromise()));
        }
      };
    }
  };
}
