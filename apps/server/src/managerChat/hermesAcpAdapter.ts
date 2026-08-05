/**
 * Hermes adapter backed by the Agent Client Protocol (ACP) -- the same
 * protocol Zed/VS Code/JetBrains use to embed Hermes as a real agent
 * component, instead of shelling out to `hermes chat -q` per turn.
 *
 * Why this replaced the CLI-spawn adapter: `-q` mode does not dispatch
 * Hermes's own slash commands at all (confirmed empirically -- sending
 * "/compress" through it just fed the literal text to the model, which is
 * how GAH ended up with its own reinvented /clear and /compact). ACP gives
 * us a persistent, stateful session per profile, a live
 * `available_commands_update` push of Hermes's real command list, and real
 * command dispatch (Hermes's own /reset and /compress) -- so GAH's chat
 * doesn't need to reinvent any of that.
 */

import { spawn, type ChildProcessByStdio } from 'child_process';
import type { Writable as NodeWritable, Readable as NodeReadable } from 'node:stream';
import { Writable, Readable } from 'node:stream';
import * as acp from '@zed-industries/agent-client-protocol';

export interface ManagerCommandInfo {
  name: string;
  description: string;
  argsHint?: string;
}

class HermesAcpClient implements acp.Client {
  availableCommands: ManagerCommandInfo[] = [];
  replyChunks: string[] = [];

  async requestPermission(params: acp.RequestPermissionRequest): Promise<acp.RequestPermissionResponse> {
    // Manager chat has no human in the loop for approval prompts (unlike an
    // editor session where a person is watching in real time). Fail
    // closed: pick a reject option if one exists, otherwise cancel --
    // never silently choose "allow" on someone's behalf.
    console.warn(`[managerChat] Hermes requested permission for "${params.toolCall?.title}" -- declining (no human in the loop for manager chat)`);
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
    // aren't surfaced in manager chat's reply text. usage_update is a
    // Hermes ACP extension the 0.4.5 client schema doesn't recognize yet --
    // the library logs a harmless internal parse warning for it and moves
    // on; it never reaches this method.
  }
}

interface ProfileConnection {
  process: ChildProcessByStdio<NodeWritable, NodeReadable, null>;
  connection: acp.ClientSideConnection;
  client: HermesAcpClient;
  sessionId: string;
  ready: Promise<void>;
}

const connections = new Map<string, ProfileConnection>();

async function connect(gahProfile: string): Promise<ProfileConnection> {
  const existing = connections.get(gahProfile);
  if (existing) {
    await existing.ready;
    return existing;
  }

  const client = new HermesAcpClient();
  // Same hardcoded Hermes profile as the prior CLI adapter -- one Hermes
  // identity across all GAH profiles/projects (matches "one manager for
  // all projects is fine" from the original scoping conversation).
  const child = spawn('hermes', ['-p', 'gah-manager', 'acp'], {
    stdio: ['pipe', 'pipe', 'inherit']
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
    ready: Promise.resolve()
  };
  state.ready = (async () => {
    await connection.initialize({
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } }
    });
    const session = await connection.newSession({ cwd: process.cwd(), mcpServers: [] });
    state.sessionId = session.sessionId;
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

export async function runHermesTurn(gahProfile: string, message: string): Promise<{ reply: string }> {
  const state = await connect(gahProfile);
  state.client.replyChunks = [];
  const result = await state.connection.prompt({
    sessionId: state.sessionId,
    prompt: [{ type: 'text', text: message }]
  });
  if (result.stopReason !== 'end_turn') {
    console.warn(`[managerChat] Hermes turn ended with stopReason=${result.stopReason} for profile ${gahProfile}`);
  }
  return { reply: state.client.replyChunks.join('') };
}

/** Backs the "/" command palette. Lazily connects (same session that'll be
 * reused for the first real message) if none exists yet for this profile. */
export async function listHermesCommands(gahProfile: string): Promise<ManagerCommandInfo[]> {
  const state = await connect(gahProfile);
  return state.client.availableCommands;
}
