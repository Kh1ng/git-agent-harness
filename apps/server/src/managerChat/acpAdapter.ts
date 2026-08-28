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

export interface SpawnSpec {
  command: string;
  args: string[];
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
  toolCalls = new Map<string, acp.ToolCallUpdate>();
  usageUpdate: acp.UsageUpdate | null = null;
  cumulativeUsage: acp.Usage | null = null;
  sessionCostUsd = 0;
  configOptions: acp.SessionConfigOption[] = [];

  constructor(private readonly label: string) {}

  async requestPermission(params: acp.RequestPermissionRequest): Promise<acp.RequestPermissionResponse> {
    // Manager chat has no human in the loop for approval prompts (unlike an
    // editor session where a person is watching in real time). Fail
    // closed: pick a reject option if one exists, otherwise cancel --
    // never silently choose "allow" on someone's behalf.
    console.warn(`[managerChat] ${this.label} requested permission for "${params.toolCall?.title}" -- declining (no human in the loop for manager chat)`);
    const reject = params.options.find((o) => o.kind === 'reject_once') ?? params.options.find((o) => o.kind === 'reject_always');
    if (reject) {
      return { outcome: { outcome: 'selected', optionId: reject.optionId } };
    }
    return { outcome: { outcome: 'cancelled' } };
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

interface ProfileConnection {
  process: ChildProcessByStdio<NodeWritable, NodeReadable, null>;
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
  knownHistory: ChatTranscriptTurn[];
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

/** ACP RPC failures reject with a plain `{ code, message, data? }` object,
 * not an `Error` instance -- and the useful detail (e.g. "You've hit your
 * usage limit...") is nested in `data.message`, while the top-level
 * `message` is often a generic "Internal error". Unwrap it into a real
 * Error with the actual detail, so it doesn't get lost by the time it
 * reaches the chat UI. */
function unwrapAcpError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (error && typeof error === 'object' && 'message' in error) {
    const data = 'data' in error ? (error as { data?: unknown }).data : undefined;
    const detail = data && typeof data === 'object' && 'message' in data ? String((data as { message: unknown }).message) : undefined;
    return new Error(detail ?? String((error as { message: unknown }).message));
  }
  return new Error(String(error));
}

export function hermesSpawnSpec(): SpawnSpec {
  return { command: 'hermes', args: ['acp'] };
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

/** Builds the runTurn/listCommands/listModels/setModel surface for one
 * backend, each backend keeping its own per-profile connection map --
 * connections are never shared across backends. */
export function createAcpBackend(label: string, spawnSpec: () => SpawnSpec) {
  const connections = new Map<string, ProfileConnection>();

  function updateModels(state: ProfileConnection, options: acp.SessionConfigOption[]): void {
    const model = readModelConfig(options);
    state.models = model.models;
    state.currentModelId = model.currentModelId;
    state.modelConfigId = model.configId;
  }

  async function selectModel(state: ProfileConnection, modelId: string): Promise<void> {
    if (!state.modelConfigId) throw new Error(`${label} does not advertise model selection.`);
    const response = await state.connection.setSessionConfigOption({
      sessionId: state.sessionId,
      configId: state.modelConfigId,
      value: modelId
    });
    state.client.configOptions = response.configOptions;
    updateModels(state, response.configOptions);
  }

  async function startSession(state: ProfileConnection, preserveModel = false): Promise<void> {
    const previousModel = preserveModel ? state.currentModelId : null;
    state.client.sessionCostUsd = 0;
    state.client.usageUpdate = null;
    state.client.cumulativeUsage = null;
    const session = await state.connection.newSession({ cwd: state.cwd, mcpServers: [] });
    state.sessionId = session.sessionId;
    state.client.configOptions = session.configOptions ?? [];
    updateModels(state, state.client.configOptions);
    if (previousModel && previousModel !== state.currentModelId && state.models.some((model) => model.id === previousModel)) {
      await selectModel(state, previousModel);
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
    const child = spawn(spec.command, spec.args, { stdio: ['pipe', 'pipe', 'inherit'] });

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
      knownHistory: [],
      ready: Promise.resolve()
    };
    state.ready = (async () => {
      await connection.initialize({
        protocolVersion: acp.PROTOCOL_VERSION,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } }
      });
      await startSession(state);
    })();

    child.on('exit', () => {
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
    }
  ): Promise<{ reply: string; model: string | null; usage: ChatUsage | null }> {
    const state = await connect(gahProfile, input.cwd);
    if (input.model && input.model !== state.currentModelId && state.models.some((m) => m.id === input.model)) {
      await selectModel(state, input.model);
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
    } catch (error) {
      throw unwrapAcpError(error);
    } finally {
      state.client.onReplyChunk = undefined;
      state.client.onToolResult = undefined;
    }
    if (result.stopReason !== 'end_turn') {
      console.warn(`[managerChat] ${label} turn ended with stopReason=${result.stopReason} for profile ${gahProfile}`);
    }
    updateModels(state, state.client.configOptions);
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

  async function listModels(gahProfile: string): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }> {
    const state = await connect(gahProfile);
    return { models: state.models, currentModelId: state.currentModelId };
  }

  async function setModel(gahProfile: string, modelId: string): Promise<void> {
    const state = await connect(gahProfile);
    if (!state.models.some((m) => m.id === modelId)) {
      throw new Error(`Unknown model "${modelId}" for ${label}`);
    }
    await selectModel(state, modelId);
  }

  /** Stops the in-flight prompt turn for a profile by sending session/cancel
   * (#960). The ACP session itself survives -- only the current turn is
   * aborted, and the same connection is reused for the next turn. */
  async function cancelTurn(gahProfile: string): Promise<void> {
    const state = connections.get(gahProfile);
    if (!state || !state.sessionId) return;
    await state.connection.cancel({ sessionId: state.sessionId });
  }

  return { runTurn, listCommands, listModels, setModel, cancelTurn };
}
