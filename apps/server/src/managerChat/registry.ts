/**
 * Manager backend registry for the manager-chat MVP.
 *
 * Hermes and OpenCode speak ACP natively; Codex and Claude use their
 * official ACP bridge packages. Vibe and AGY use the headless adapter,
 * which preserves transcript replay but does not advertise live config
 * options such as model or reasoning-effort selection.
 */

import {
  createAcpBackend,
  hermesSpawnSpec,
  codexSpawnSpec,
  claudeSpawnSpec,
  opencodeSpawnSpec,
  type ManagerCommandInfo,
  type ManagerModelInfo,
  type ManagerReasoningEffortInfo
} from './acpAdapter.js';
import { createHeadlessBackend, vibeBackendSpec, agyBackendSpec } from './headlessAdapter.js';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';

export type { ManagerCommandInfo, ManagerModelInfo, ManagerReasoningEffortInfo };

export interface ManagerBackendInfo {
  id: string;
  displayName: string;
  implemented: boolean;
}

export interface ManagerAdapter extends ManagerBackendInfo {
  runTurn(
    gahProfile: string,
    input: {
      prompt: string;
      history: ChatTranscriptTurn[];
      onChunk: (text: string) => void;
      onToolResult: (name: string, text: string) => void;
      /** Session working directory (WP2); omitted = the server's cwd. */
      cwd?: string;
      /** Model override for this conversation (WP2 sessions). */
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
      /** Permission round-trip (slice 3). */
      requestPermission?: (request: {
        title: string;
        options: { optionId: string; name: string; kind: string }[];
        locations: string[];
      }) => Promise<string>;
    }
  ): Promise<{ reply: string; model: string | null; usage: ChatUsage | null }>;
  listCommands(gahProfile: string): Promise<ManagerCommandInfo[]>;
  listModels(gahProfile: string): Promise<{
    models: ManagerModelInfo[];
    currentModelId: string | null;
    reasoningEfforts: ManagerReasoningEffortInfo[];
    currentReasoningEffortId: string | null;
  }>;
  setModel(gahProfile: string, modelId: string): Promise<void>;
  setReasoningEffort(gahProfile: string, effortId: string): Promise<void>;
  steerTurn(gahProfile: string, message: string): Promise<{ outcome: 'injected' }>;
  cancelTurn(gahProfile: string): Promise<void>;
}

class NotImplementedAdapter implements ManagerAdapter {
  implemented = false;
  constructor(
    public id: string,
    public displayName: string,
    private trackingIssue: string
  ) {}

  async runTurn(): Promise<{ reply: string; model: string | null; usage: ChatUsage | null }> {
    throw new Error(
      `${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}). Pick a different backend in Settings.`
    );
  }

  async listCommands(): Promise<ManagerCommandInfo[]> {
    return [];
  }

  async listModels() {
    return { models: [], currentModelId: null, reasoningEfforts: [], currentReasoningEffortId: null };
  }

  async setModel(): Promise<void> {
    throw new Error(`${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}).`);
  }

  async setReasoningEffort(): Promise<void> {
    throw new Error(`${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}).`);
  }

  async cancelTurn(): Promise<void> {
    throw new Error(`${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}).`);
  }

  async steerTurn(): Promise<{ outcome: 'injected' }> {
    throw new Error(`${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}).`);
  }
}

function acpManagerAdapter(
  id: string,
  displayName: string,
  spawnSpec: () => import('./acpAdapter.js').SpawnSpec,
  options?: { nativeSteering?: boolean; consecutiveFailureReconnectThreshold?: number }
): ManagerAdapter {
  const backend = createAcpBackend(displayName, spawnSpec, options);
  return { id, displayName, implemented: true, ...backend };
}

const REGISTRY: Record<string, ManagerAdapter> = {
  hermes: acpManagerAdapter('hermes', 'Hermes', hermesSpawnSpec),
  codex: acpManagerAdapter('codex', 'Codex', codexSpawnSpec, { consecutiveFailureReconnectThreshold: 2 }),
  // claude-agent-acp currently lets an injected steer outlive and detach
  // from the owning session/prompt (agentclientprotocol/claude-agent-acp#934).
  // Do not advertise that unsafe path until the adapter preserves lifecycle.
  claude: acpManagerAdapter('claude', 'Claude', claudeSpawnSpec, { nativeSteering: false }),
  opencode: acpManagerAdapter('opencode', 'OpenCode', opencodeSpawnSpec),
  vibe: { ...createHeadlessBackend(vibeBackendSpec()) } as ManagerAdapter,
  agy: { ...createHeadlessBackend(agyBackendSpec()) } as ManagerAdapter
};

export const DEFAULT_BACKEND_ID = 'hermes';

export function listManagerBackends(): ManagerBackendInfo[] {
  return Object.values(REGISTRY).map(({ id, displayName, implemented }) => ({ id, displayName, implemented }));
}

export function resolveAdapter(backendId: string): ManagerAdapter {
  const adapter = REGISTRY[backendId];
  if (!adapter) {
    throw new Error(`Unknown manager backend "${backendId}"`);
  }
  return adapter;
}
