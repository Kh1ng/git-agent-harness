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
import * as acp from '@zed-industries/agent-client-protocol';
import type { Writable as NodeWritable, Readable as NodeReadable } from 'node:stream';

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

class AcpClient implements acp.Client {
  availableCommands: ManagerCommandInfo[] = [];
  replyChunks: string[] = [];

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
    } else if (update.sessionUpdate === 'available_commands_update') {
      this.availableCommands = update.availableCommands.map((cmd) => ({
        name: cmd.name,
        description: cmd.description,
        argsHint: cmd.input && 'hint' in cmd.input ? cmd.input.hint : undefined
      }));
    }
    // agent_thought_chunk / tool_call / tool_call_update / plan updates
    // aren't surfaced in manager chat's reply text. usage_update and
    // session_info_update are agent-side ACP extensions the 0.4.5 client
    // schema doesn't recognize yet -- the library logs a harmless internal
    // parse warning for them and moves on; they never reach this method.
  }
}

interface ProfileConnection {
  process: ChildProcessByStdio<NodeWritable, NodeReadable, null>;
  connection: acp.ClientSideConnection;
  client: AcpClient;
  sessionId: string;
  models: ManagerModelInfo[];
  currentModelId: string | null;
  // ACP session *modes* (Default / Accept Edits / Don't Ask) are unrelated
  // to model selection per spec -- but Hermes's ACP server has a bug where
  // its session/set_model handler requires a `modeId` field that isn't
  // part of the standard SetSessionModelRequest schema at all (confirmed:
  // omitting it fails server-side with "Field required: modeId"; Codex's
  // and Claude's bridges don't have this quirk). Track it so setModel() can
  // work around it for whichever backend needs it, without affecting
  // backends that don't have `modes` (the field is naturally omitted).
  currentModeId: string | null;
  ready: Promise<void>;
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
  // Same hardcoded Hermes profile as the original CLI adapter -- one Hermes
  // identity across all GAH profiles/projects (matches "one manager for all
  // projects is fine" from the original scoping conversation).
  return { command: 'hermes', args: ['-p', 'gah-manager', 'acp'] };
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

  async function connect(gahProfile: string): Promise<ProfileConnection> {
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
      models: [],
      currentModelId: null,
      currentModeId: null,
      ready: Promise.resolve()
    };
    state.ready = (async () => {
      await connection.initialize({
        protocolVersion: acp.PROTOCOL_VERSION,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } }
      });
      const session = await connection.newSession({ cwd: process.cwd(), mcpServers: [] });
      state.sessionId = session.sessionId;
      // **UNSTABLE** per the ACP spec -- not every agent returns this (Claude's
      // bridge doesn't today). Absence just means no model picker for that
      // backend, not an error.
      if (session.models) {
        state.models = session.models.availableModels.map((m) => ({ id: m.modelId, name: m.name, description: m.description ?? undefined }));
        state.currentModelId = session.models.currentModelId;
      }
      if (session.modes) {
        state.currentModeId = session.modes.currentModeId;
      }
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

  async function runTurn(gahProfile: string, message: string): Promise<{ reply: string }> {
    const state = await connect(gahProfile);
    state.client.replyChunks = [];
    let result;
    try {
      result = await state.connection.prompt({
        sessionId: state.sessionId,
        prompt: [{ type: 'text', text: message }]
      });
    } catch (error) {
      throw unwrapAcpError(error);
    }
    if (result.stopReason !== 'end_turn') {
      console.warn(`[managerChat] ${label} turn ended with stopReason=${result.stopReason} for profile ${gahProfile}`);
    }
    return { reply: state.client.replyChunks.join('') };
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
    // modeId isn't part of the standard SetSessionModelRequest -- it's the
    // Hermes-server-bug workaround (see the ProfileConnection.currentModeId
    // comment above). Safe no-op for backends without modes: currentModeId
    // is null there, and JSON.stringify drops undefined keys entirely.
    await state.connection.setSessionModel({
      sessionId: state.sessionId,
      modelId,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- see comment above, this is a deliberate off-spec field
      ...(state.currentModeId ? { modeId: state.currentModeId } : {})
    } as any);
    state.currentModelId = modelId;
  }

  return { runTurn, listCommands, listModels, setModel };
}
