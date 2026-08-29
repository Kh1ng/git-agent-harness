/**
 * Shared Agent Client Protocol (ACP) client -- the same protocol Zed/VS
 * Code/JetBrains use to embed coding agents as real components, instead of
 * shelling out to a one-shot CLI query per turn (which doesn't dispatch a
 * backend's own slash commands at all -- confirmed empirically for Hermes's
 * `-q` mode, which fed "/compress" to the model as literal text instead of
 * running the real command).
 *
 * Hermes speaks ACP natively (`hermes acp`). Codex and Claude Code don't,
 * but both have official Zed-maintained ACP bridge packages
 * (@agentclientprotocol/codex-acp, @agentclientprotocol/claude-agent-acp)
 * that wrap them as ACP agents -- so all three backends use this exact same
 * connection lifecycle, parameterized only by which process to spawn.
 */

import { spawn, type ChildProcessByStdio } from 'child_process';
import { createRequire } from 'node:module';
import path from 'node:path';
import { Writable, Readable } from 'node:stream';
import * as acp from '@agentclientprotocol/sdk';
import type { Writable as NodeWritable, Readable as NodeReadable } from 'node:stream';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';

export interface ManagerCommandInfo {
  name: string;
  description: string;
  argsHint?: string;
}

export interface ManagerModelInfo {
  id: string;
  name: string;
  description?: string;
}

export interface ManagerReasoningEffortInfo {
  id: string;
  name: string;
  description?: string;
}

export interface SpawnSpec {
  command: string;
  args: string[];
  env?: Record<string, string>;
}

const COMPACTION_COMMAND_NAMES = new Set(['compact', 'compress', 'clear', 'reset']);

function commandName(message: string): string | null {
  if (!message.trim().startsWith('/')) return null;
  return message.trim().slice(1).split(/\s+/)[0]?.toLowerCase() || null;
}

export function isCompactionCommand(message: string): boolean {
  const name = commandName(message);
  return name !== null && COMPACTION_COMMAND_NAMES.has(name);
}

export function compactionSummary(message: string, reply: string): string {
  const command = commandName(message);
  return command === 'clear' || command === 'reset'
    ? 'Conversation cleared.'
    : reply.trim() || 'Context compacted.';
}

class AcpClient implements acp.Client {
  availableCommands: ManagerCommandInfo[] = [];
  replyChunks: string[] = [];
  onReplyChunk?: (text: string) => void;
  onToolResult?: (name: string, text: string) => void;
  /** Structured tool-call stream (slice 3): called on every tool_call /
   * tool_call_update with the merged snapshot. The manager logs it, pushes
   * it over the WS, and renders it as activity cards. */
  onToolCall?: (tool: {
    toolCallId: string;
    name: string | null;
    title: string;
    kind: string | null;
    status: 'pending' | 'completed' | 'failed';
    locations: string[];
    summary: string | null;
  }) => void;
  /** Permission decision hook (slice 3): when set, requestPermission asks
   * the human through the chat UI instead of auto-declining; resolves with
   * the chosen optionId or 'cancelled'. */
  permissionRequest?: (request: {
    title: string;
    options: { optionId: string; name: string; kind: string }[];
    locations: string[];
  }) => Promise<string>;
  toolCalls = new Map<string, acp.ToolCallUpdate>();
  usageUpdate: acp.UsageUpdate | null = null;
  cumulativeUsage: acp.Usage | null = null;
  sessionCostUsd = 0;
  configOptions: acp.SessionConfigOption[] = [];

  constructor(private readonly label: string) {}

  async requestPermission(params: acp.RequestPermissionRequest): Promise<acp.RequestPermissionResponse> {
    // Fail closed unless a human is watching through the chat UI (slice 3).
    const title = params.toolCall?.title ?? params.toolCall?.name ?? 'Unknown action';
    if (!this.permissionRequest) {
      console.warn(`[managerChat] ${this.label} requested permission for "${title}" -- declining (no permission UI attached)`);
      const reject = params.options.find((o) => o.kind === 'reject_once') ?? params.options.find((o) => o.kind === 'reject_always');
      if (reject) {
        return { outcome: { outcome: 'selected', optionId: reject.optionId } };
      }
      return { outcome: { outcome: 'cancelled' } };
    }
    const locations = (params.toolCall?.locations ?? []).map((l) => l.path);
    const chosen = await this.permissionRequest({
      title,
      options: params.options.map((o) => ({ optionId: o.optionId, name: o.name, kind: o.kind })),
      locations
    });
    console.log(`[managerChat] ${this.label} permission "${title}" -> ${chosen}`);
    if (chosen === 'cancelled') {
      return { outcome: { outcome: 'cancelled' } };
    }
    return { outcome: { outcome: 'selected', optionId: chosen } };
  }

  async sessionUpdate(params: acp.SessionNotification): Promise<void> {
    const update = params.update;
    if (update.sessionUpdate === 'agent_message_chunk' && update.content.type === 'text') {
      this.replyChunks.push(update.content.text);
      this.onReplyChunk?.(update.content.text);
    } else if (update.sessionUpdate === 'available_commands_update') {
      this.availableCommands = update.availableCommands.map((cmd) => ({
        name: cmd.name,
        description: cmd.description,
        argsHint: cmd.input && 'hint' in cmd.input ? cmd.input.hint : undefined
      }));
    } else if (update.sessionUpdate === 'usage_update') {
      this.usageUpdate = update;
      if (update.cost?.currency === 'USD') this.sessionCostUsd = update.cost.amount;
    } else if (update.sessionUpdate === 'config_option_update') {
      this.configOptions = update.configOptions;
    } else if (update.sessionUpdate === 'tool_call' || update.sessionUpdate === 'tool_call_update') {
      const previous = this.toolCalls.get(update.toolCallId);
      const tool = { ...previous, ...update };
      this.toolCalls.set(update.toolCallId, tool);
      this.onToolCall?.({
        toolCallId: tool.toolCallId,
        name: tool.name ?? null,
        title: tool.title ?? tool.name ?? tool.toolCallId,
        kind: tool.kind ?? null,
        status: (tool.status ?? 'pending') as 'pending' | 'completed' | 'failed',
        locations: (tool.locations ?? []).map((l) => l.path),
        summary: summarizeToolOutput(tool)
      });
      const finished = tool.status === 'completed' || tool.status === 'failed';
      if (finished && previous?.status !== 'completed' && previous?.status !== 'failed') {
        const value = tool.rawOutput ?? tool.content ?? tool.status;
        this.onToolResult?.(tool.name ?? tool.title ?? tool.toolCallId, typeof value === 'string' ? value : JSON.stringify(value));
      }
    }
    // agent_thought_chunk / plan updates aren't surfaced in manager chat.
  }
}

export function toChatUsage(
  usage: acp.Usage | null | undefined,
  previousUsage: acp.Usage | null,
  update: acp.UsageUpdate | null,
  costBeforeUsd: number,
  durationMs: number
): ChatUsage | null {
  if (!usage && !update) return null;
  const cost = update?.cost?.currency === 'USD'
    ? Math.max(0, update.cost.amount - costBeforeUsd)
    : null;
  return {
    input_tokens: usage ? counterDelta(usage.inputTokens, previousUsage?.inputTokens) : null,
    output_tokens: usage ? counterDelta(usage.outputTokens, previousUsage?.outputTokens) : null,
    total_tokens: usage ? counterDelta(usage.totalTokens, previousUsage?.totalTokens) : null,
    estimated_cost_usd: cost,
    duration_seconds: durationMs / 1000
  };
}

function counterDelta(current: number, previous: number | undefined): number {
  return previous === undefined || current < previous ? current : current - previous;
}

export function readModelConfig(options: acp.SessionConfigOption[]): {
  models: ManagerModelInfo[];
  currentModelId: string | null;
  configId: string | null;
} {
  const model = options.find((option) => option.type === 'select' && (option.category === 'model' || option.id === 'model'));
  if (!model || model.type !== 'select') return { models: [], currentModelId: null, configId: null };
  const choices = model.options.flatMap((option) => 'options' in option ? option.options : [option]);
  return {
    models: choices.map((option) => ({ id: option.value, name: option.name, description: option.description ?? undefined })),
    currentModelId: model.currentValue,
    configId: model.id
  };
}

/** ACP owns the reasoning vocabulary. Only the standard thought_level
 * category is surfaced, and every value/name comes from the backend's
 * advertised select option rather than a GAH-maintained enum. */
export function readReasoningConfig(options: acp.SessionConfigOption[]): {
  efforts: ManagerReasoningEffortInfo[];
  currentEffortId: string | null;
  configId: string | null;
} {
  const reasoning = options.find((option) => option.type === 'select' && option.category === 'thought_level');
  if (!reasoning || reasoning.type !== 'select') {
    return { efforts: [], currentEffortId: null, configId: null };
  }
  const choices = reasoning.options.flatMap((option) => 'options' in option ? option.options : [option]);
  return {
    efforts: choices.map((option) => ({ id: option.value, name: option.name, description: option.description ?? undefined })),
    currentEffortId: reasoning.currentValue,
    configId: reasoning.id
  };
}

interface ProfileConnection {
  process: ChildProcessByStdio<NodeWritable, NodeReadable, NodeReadable>;
  connection: acp.ClientSideConnection;
  client: AcpClient;
  sessionId: string;
  /** Working directory for new ACP sessions. The server's cwd for the
   * default per-profile conversation; a session's worktree for session-bound
   * turns (WP2) -- which is what makes a worktree interchangeable between
   * backends: every backend spawns into the same directory. */
  cwd: string;
  models: ManagerModelInfo[];
  currentModelId: string | null;
  modelConfigId: string | null;
  reasoningEfforts: ManagerReasoningEffortInfo[];
  currentReasoningEffortId: string | null;
  reasoningConfigId: string | null;
  steeringSupported: boolean;
  knownHistory: ChatTranscriptTurn[];
  consecutiveFailures: number;
  stderrTail: Buffer;
  exit: { code: number | null; signal: NodeJS.Signals | null } | null;
  ready: Promise<void>;
}

/** Rehydrate a fresh ACP session from the durable transcript. */
export function resumePrompt(prompt: string, history: ChatTranscriptTurn[]): string {
  if (history.length === 0) return prompt;
  const transcript = history.map((turn) => `${turn.role}: ${turn.text}`).join('\n');
  return `Resume this conversation and answer only the final user message.\n\n${transcript}\n\nuser: ${prompt}`;
}

export function historyDelta(
  known: ChatTranscriptTurn[],
  current: ChatTranscriptTurn[]
): ChatTranscriptTurn[] | null {
  const matches = known.every(
    (turn, index) => turn.role === current[index]?.role && turn.text === current[index]?.text
  );
  return matches ? current.slice(known.length) : null;
}

/** Resolves an npm package's `bin` script to an absolute path -- these
 * bridge packages aren't installed globally like `hermes`/`codex`/`claude`
 * are, so PATH resolution won't find them. */
function resolveBinScript(packageName: string): string {
  const require = createRequire(import.meta.url);
  const pkgJsonPath = require.resolve(`${packageName}/package.json`);
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const pkg = require(pkgJsonPath) as { bin?: string | Record<string, string> };
  const binRelative = typeof pkg.bin === 'string' ? pkg.bin : Object.values(pkg.bin ?? {})[0];
  if (!binRelative) {
    throw new Error(`${packageName} has no "bin" entry in package.json`);
  }
  return path.resolve(path.dirname(pkgJsonPath), binRelative);
}

/** ACP RPC failures carry `{ code, message, data? }` fields, either as a
 * plain object or an Error materialized by the SDK. The useful detail (e.g.
 * "You've hit your usage limit...") is nested in `data.message`, while the
 * top-level `message` is often a generic "Internal error". */
const ACP_STDERR_TAIL_BYTES = 4_096;
const ACP_STDERR_RENDER_CHARS = 2_000;

function appendStderrTail(current: Buffer, chunk: Buffer | string): Buffer {
  const incoming = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
  if (incoming.length >= ACP_STDERR_TAIL_BYTES) return incoming.subarray(incoming.length - ACP_STDERR_TAIL_BYTES);
  const combined = Buffer.concat([current, incoming]);
  return combined.length > ACP_STDERR_TAIL_BYTES
    ? combined.subarray(combined.length - ACP_STDERR_TAIL_BYTES)
    : combined;
}

function childDiagnostics(state: ProfileConnection): string {
  const stderr = state.stderrTail
    .toString('utf8')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(-ACP_STDERR_RENDER_CHARS);
  const observedExit = state.exit ?? (
    state.process.exitCode !== null || state.process.signalCode !== null
      ? { code: state.process.exitCode, signal: state.process.signalCode }
      : null
  );
  const exit = observedExit
    ? `child exit=${observedExit.code ?? 'null'} signal=${observedExit.signal ?? 'none'}`
    : 'child=running';
  return `${exit}; stderr tail=${stderr ? JSON.stringify(stderr) : '<empty>'}`;
}

function nestedAcpErrorDetail(error: unknown): string | undefined {
  if (!error || typeof error !== 'object' || !('data' in error)) return undefined;
  const data = (error as { data?: unknown }).data;
  return data && typeof data === 'object' && 'message' in data
    ? String((data as { message: unknown }).message)
    : undefined;
}

function unwrapAcpError(error: unknown, state: ProfileConnection): Error {
  if (error && typeof error === 'object' && 'message' in error) {
    const detail = nestedAcpErrorDetail(error);
    if (detail !== undefined) return new Error(detail);
    const code = 'code' in error ? String((error as { code?: unknown }).code ?? 'unknown') : 'unknown';
    return new Error(`${String((error as { message: unknown }).message)} [ACP code=${code}; ${childDiagnostics(state)}]`);
  }
  return new Error(String(error));
}

export function hermesSpawnSpec(): SpawnSpec {
  return { command: 'hermes', args: ['acp'] };
}

/** A short, single-line digest of a finished tool call's output for the
 * activity card: prefers a text content block, falls back to rawOutput.
 * Bounded so a huge tool output can't flood the log/WS. */
function summarizeToolOutput(tool: acp.ToolCallUpdate): string | null {
  if (tool.status !== 'completed' && tool.status !== 'failed') return null;
  const textBlock = tool.content?.find(
    (c): c is Extract<typeof c, { type: 'content' }> =>
      c.type === 'content' && c.content.type === 'text'
  );
  const text = textBlock?.content.type === 'text' ? textBlock.content.text : undefined;
  const source = text ?? (tool.rawOutput != null
    ? (typeof tool.rawOutput === 'string' ? tool.rawOutput : JSON.stringify(tool.rawOutput))
    : null);
  if (source == null) return tool.status;
  return source.split('\n').slice(0, 5).join('\n').slice(0, 400);
}

/** Classifies whether an adapter error is a usage/quota limit (#962). The
 * unwrapped error already carries the nested `data.message` detail (e.g.
 * "You've hit your usage limit..."), so matching on the message is the
 * trigger signal. Anything else -- auth, backend crash, network -- is never
 * a handoff trigger. */
export function isUsageLimitError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  // Quota/usage-limit triggers only. Deliberately not matching "token limit"
  // (a max_tokens stop, not a quota) or generic "limit reached" phrasing.
  return /usage limit|rate limit|quota|exhausted|insufficient (credits|quota)|hit (your|the) (daily |monthly )?limit|quota.*exceed/i.test(message);
}

export function codexSpawnSpec(): SpawnSpec {
  return { command: 'node', args: [resolveBinScript('@agentclientprotocol/codex-acp')] };
}

export function claudeSpawnSpec(): SpawnSpec {
  return { command: 'node', args: [resolveBinScript('@agentclientprotocol/claude-agent-acp')] };
}

/** opencode ships a native ACP server (`opencode acp`) — Tier A like
 * Hermes, no bridge needed. */
export function opencodeSpawnSpec(): SpawnSpec {
  const inherited = process.env.OPENCODE_CONFIG_CONTENT;
  let config: Record<string, unknown> = {};
  if (inherited !== undefined) {
    try {
      const parsed: unknown = JSON.parse(inherited);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('expected a JSON object');
      config = parsed as Record<string, unknown>;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw new Error(`Cannot start OpenCode Manager Chat: OPENCODE_CONFIG_CONTENT must be a JSON object (${detail}).`);
    }
  }
  return {
    command: 'opencode',
    args: ['acp'],
    env: { OPENCODE_CONFIG_CONTENT: JSON.stringify({ ...config, default_agent: 'gah-implementer' }) }
  };
}

/** Builds the runTurn/listCommands/listModels/setModel surface for one
 * backend, each backend keeping its own per-profile connection map --
 * connections are never shared across backends. */
export function createAcpBackend(
  label: string,
  spawnSpec: () => SpawnSpec,
  options: { nativeSteering?: boolean; consecutiveFailureReconnectThreshold?: number } = {}
) {
  const connections = new Map<string, ProfileConnection>();

  function updateSessionConfig(state: ProfileConnection, options: acp.SessionConfigOption[]): void {
    const model = readModelConfig(options);
    state.models = model.models;
    state.currentModelId = model.currentModelId;
    state.modelConfigId = model.configId;
    const reasoning = readReasoningConfig(options);
    state.reasoningEfforts = reasoning.efforts;
    state.currentReasoningEffortId = reasoning.currentEffortId;
    state.reasoningConfigId = reasoning.configId;
  }

  async function selectConfigOption(state: ProfileConnection, configId: string, value: string): Promise<void> {
    const response = await state.connection.setSessionConfigOption({
      sessionId: state.sessionId,
      configId,
      value
    });
    state.client.configOptions = response.configOptions;
    updateSessionConfig(state, response.configOptions);
  }

  async function selectModel(state: ProfileConnection, modelId: string): Promise<void> {
    if (!state.modelConfigId) throw new Error(`${label} does not advertise model selection.`);
    await selectConfigOption(state, state.modelConfigId, modelId);
  }

  async function selectReasoningEffort(state: ProfileConnection, effortId: string): Promise<void> {
    if (!state.reasoningConfigId) throw new Error(`${label} does not advertise reasoning-effort selection.`);
    await selectConfigOption(state, state.reasoningConfigId, effortId);
  }

  async function startSession(state: ProfileConnection, preserveConfig = false): Promise<void> {
    const previousModel = preserveConfig ? state.currentModelId : null;
    const previousReasoningEffort = preserveConfig ? state.currentReasoningEffortId : null;
    state.client.sessionCostUsd = 0;
    state.client.usageUpdate = null;
    state.client.cumulativeUsage = null;
    const session = await state.connection.newSession({ cwd: state.cwd, mcpServers: [] });
    state.sessionId = session.sessionId;
    state.client.configOptions = session.configOptions ?? [];
    updateSessionConfig(state, state.client.configOptions);
    if (previousModel && previousModel !== state.currentModelId && state.models.some((model) => model.id === previousModel)) {
      await selectModel(state, previousModel);
    }
    if (
      previousReasoningEffort
      && previousReasoningEffort !== state.currentReasoningEffortId
      && state.reasoningEfforts.some((effort) => effort.id === previousReasoningEffort)
    ) {
      await selectReasoningEffort(state, previousReasoningEffort);
    }
  }

  async function connect(gahProfile: string, cwd?: string): Promise<ProfileConnection> {
    const existing = connections.get(gahProfile);
    if (existing) {
      await existing.ready;
      return existing;
    }

    const client = new AcpClient(label);
    const spec = spawnSpec();
    const child = spawn(spec.command, spec.args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: spec.env ? { ...process.env, ...spec.env } : undefined
    });

    const stream = acp.ndJsonStream(
      Writable.toWeb(child.stdin) as WritableStream<Uint8Array>,
      Readable.toWeb(child.stdout) as ReadableStream<Uint8Array>
    );
    const connection = new acp.ClientSideConnection(() => client, stream);

    const state: ProfileConnection = {
      process: child,
      connection,
      client,
      sessionId: '',
      cwd: cwd ?? process.cwd(),
      models: [],
      currentModelId: null,
      modelConfigId: null,
      reasoningEfforts: [],
      currentReasoningEffortId: null,
      reasoningConfigId: null,
      steeringSupported: false,
      knownHistory: [],
      consecutiveFailures: 0,
      stderrTail: Buffer.alloc(0),
      exit: null,
      ready: Promise.resolve()
    };
    child.stderr.on('data', (chunk: Buffer | string) => {
      state.stderrTail = appendStderrTail(state.stderrTail, chunk);
      process.stderr.write(chunk);
    });
    state.ready = (async () => {
      const initialized = await connection.initialize({
        protocolVersion: acp.PROTOCOL_VERSION,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } }
      });
      const steering = initialized._meta?.steering as { supported?: unknown } | undefined;
      state.steeringSupported = options.nativeSteering !== false && steering?.supported === true;
      await startSession(state);
    })();

    child.on('exit', (code, signal) => {
      state.exit = { code, signal };
      if (connections.get(gahProfile) === state) {
        connections.delete(gahProfile);
      }
    });

    connections.set(gahProfile, state);
    await state.ready;
    return state;
  }

  async function runTurn(
    gahProfile: string,
    input: {
      prompt: string;
      history: ChatTranscriptTurn[];
      onChunk: (text: string) => void;
      onToolResult: (name: string, text: string) => void;
      /** Working directory for this conversation's session (WP2): the
       * profile default omits it and gets the server cwd; a session passes
       * its worktree path. Only the FIRST connect per key wins -- a
       * connection is reused across turns and its cwd is immutable, which
       * is correct: one conversation = one directory. */
      cwd?: string;
      /** Model override for this conversation (WP2 sessions): applied after
       * connect (never before -- setModel lazily connects, which would race
       * the cwd first-connect rule), then only on change. Backends without
       * a model picker (e.g. Claude's bridge) throw in selectModel; that's
       * expected and degrades to the backend's default. */
      model?: string | null;
      /** Structured tool-call stream (slice 3). */
      onToolCall?: (tool: {
        toolCallId: string;
        name: string | null;
        title: string;
        kind: string | null;
        status: 'pending' | 'completed' | 'failed';
        locations: string[];
        summary: string | null;
      }) => void;
      /** Permission round-trip (slice 3): resolve with the chosen optionId
       * or 'cancelled'. Absent = fail-closed auto-decline (previous
       * behavior, e.g. for adapter-driven turns without a UI attached). */
      requestPermission?: (request: {
        title: string;
        options: { optionId: string; name: string; kind: string }[];
        locations: string[];
      }) => Promise<string>;
      /** Per-session reasoning effort (WP2 sessions); undefined = none.
       * Applied like model: only when the backend advertises the
       * thought_level control and the value differs from current. */
      reasoningEffort?: string | null;
    }
  ): Promise<{ reply: string; model: string | null; usage: ChatUsage | null }> {
    const state = await connect(gahProfile, input.cwd);
    if (input.model && input.model !== state.currentModelId && state.models.some((m) => m.id === input.model)) {
      await selectModel(state, input.model);
    }
    if (
      input.reasoningEffort
      && input.reasoningEffort !== state.currentReasoningEffortId
      && state.reasoningEfforts.some((effort) => effort.id === input.reasoningEffort)
    ) {
      await selectReasoningEffort(state, input.reasoningEffort);
    }
    state.client.replyChunks = [];
    state.client.toolCalls.clear();
    let missingHistory = historyDelta(state.knownHistory, input.history);
    if (missingHistory === null) {
      await startSession(state, true);
      state.knownHistory = [];
      missingHistory = input.history;
    }
    const slashCommand = commandName(input.prompt) !== null;
    const command = commandName(input.prompt);
    // ponytail: ACP cannot inject roleful history ahead of a native command;
    // use loadSession when adapters persist their native session ids.
    if ((command === 'compact' || command === 'compress') && missingHistory.length > 0) {
      throw new Error('Send one normal message to restore this backend before compacting its context.');
    }
    // Slice 3 hooks: attach per-turn (cleared in the finally below) so the
    // connection's client streams structured tool calls and permission
    // requests to whoever is watching this turn.
    state.client.onToolCall = input.onToolCall;
    state.client.permissionRequest = input.requestPermission;
    const prompt = slashCommand ? input.prompt : resumePrompt(input.prompt, missingHistory);
    state.client.onReplyChunk = input.onChunk;
    state.client.onToolResult = input.onToolResult;
    state.client.usageUpdate = null;
    const costBeforeUsd = state.client.sessionCostUsd;
    const startedAt = Date.now();
    let result;
    try {
      result = await state.connection.prompt({
        sessionId: state.sessionId,
        prompt: [{ type: 'text', text: prompt }]
      });
      state.consecutiveFailures = 0;
    } catch (error) {
      state.consecutiveFailures += 1;
      const threshold = options.consecutiveFailureReconnectThreshold;
      const evicted = threshold !== undefined && state.consecutiveFailures >= threshold;
      if (evicted && connections.get(gahProfile) === state) {
        connections.delete(gahProfile);
        state.process.kill();
      }
      // stdout and stderr are separate pipes. Give stderr already emitted by
      // the child one event-loop turn to reach its bounded tail before the
      // rejection is formatted for the durable harness/error event.
      await new Promise<void>((resolve) => setImmediate(resolve));
      const failure = unwrapAcpError(error, state);
      if (threshold !== undefined && nestedAcpErrorDetail(error) === undefined) {
        failure.message += ` [consecutive failures=${state.consecutiveFailures}/${threshold}${evicted ? '; connection evicted' : ''}]`;
      }
      throw failure;
    } finally {
      state.client.onReplyChunk = undefined;
      state.client.onToolResult = undefined;
      state.client.onToolCall = undefined;
      state.client.permissionRequest = undefined;
    }
    if (result.stopReason !== 'end_turn') {
      console.warn(`[managerChat] ${label} turn ended with stopReason=${result.stopReason} for profile ${gahProfile}`);
    }
    updateSessionConfig(state, state.client.configOptions);
    const reply = state.client.replyChunks.join('');
    if (isCompactionCommand(input.prompt)) {
      state.knownHistory = [{ role: 'system', text: compactionSummary(input.prompt, reply), timestamp: Date.now() }];
    } else if (!slashCommand) {
      state.knownHistory = [
        ...input.history,
        { role: 'user', text: input.prompt, timestamp: Date.now() },
        { role: 'assistant', text: reply, timestamp: Date.now() }
      ];
    }
    const usage = toChatUsage(
      result.usage,
      state.client.cumulativeUsage,
      state.client.usageUpdate,
      costBeforeUsd,
      Date.now() - startedAt
    );
    state.client.cumulativeUsage = result.usage ?? state.client.cumulativeUsage;
    return {
      reply,
      model: state.currentModelId,
      usage
    };
  }

  /** Backs the "/" command palette. Lazily connects (the same session
   * that'll be reused for the first real message) if none exists yet. */
  async function listCommands(gahProfile: string): Promise<ManagerCommandInfo[]> {
    const state = await connect(gahProfile);
    return state.client.availableCommands;
  }

  async function listModels(gahProfile: string) {
    const state = await connect(gahProfile);
    return {
      models: state.models,
      currentModelId: state.currentModelId,
      reasoningEfforts: state.reasoningEfforts,
      currentReasoningEffortId: state.currentReasoningEffortId
    };
  }

  async function setModel(gahProfile: string, modelId: string): Promise<void> {
    const state = await connect(gahProfile);
    if (!state.models.some((m) => m.id === modelId)) {
      throw new Error(`Unknown model "${modelId}" for ${label}`);
    }
    await selectModel(state, modelId);
  }

  async function setReasoningEffort(gahProfile: string, effortId: string): Promise<void> {
    const state = await connect(gahProfile);
    if (!state.reasoningEfforts.some((effort) => effort.id === effortId)) {
      throw new Error(`Unknown reasoning effort "${effortId}" for ${label}`);
    }
    await selectReasoningEffort(state, effortId);
  }

  async function steerTurn(gahProfile: string, message: string): Promise<{ outcome: 'injected' }> {
    const state = connections.get(gahProfile);
    if (!state || !state.sessionId) throw new Error(`No active ${label} session to steer.`);
    if (!state.steeringSupported) throw new Error(`${label} does not support mid-turn steering.`);
    const result = await state.connection.extMethod('_session/steering', {
      sessionId: state.sessionId,
      prompt: [{ type: 'text', text: message }],
      _meta: { steering: { idleBehavior: 'promptRequired' } }
    });
    if (result.outcome !== 'injected') {
      // The Codex ACP extension may race the original turn ending and start
      // the steer as a new turn. GAH only supports true mid-turn injection:
      // stop that untracked turn before reporting the failed steer.
      if (result.outcome === 'startedNewTurn') {
        await state.connection.cancel({ sessionId: state.sessionId });
      }
      throw new Error(`The ${label} turn ended before the steering message could be injected.`);
    }
    return { outcome: 'injected' };
  }

  /** Stops the in-flight prompt turn for a profile by sending session/cancel
   * (#960). The ACP session itself survives -- only the current turn is
   * aborted, and the same connection is reused for the next turn. */
  async function cancelTurn(gahProfile: string): Promise<void> {
    const state = connections.get(gahProfile);
    if (!state || !state.sessionId) return;
    await state.connection.cancel({ sessionId: state.sessionId });
  }

  return { runTurn, listCommands, listModels, setModel, setReasoningEffort, steerTurn, cancelTurn };
}
