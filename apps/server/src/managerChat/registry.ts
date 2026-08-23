/**
 * Manager backend registry for the manager-chat MVP.
 *
 * Hermes, Codex, and Claude all have real ACP-backed adapters (see
 * acpAdapter.ts -- Hermes speaks ACP natively, Codex/Claude via their
 * official Zed-maintained bridge packages). Vibe/opencode/agy have no ACP
 * or equivalent structured protocol, so they stay unimplemented for manager
 * chat -- picking one fails loudly with a clear message instead of the chat
 * silently doing nothing or falling back to a different backend.
 */

import { createAcpBackend, hermesSpawnSpec, codexSpawnSpec, claudeSpawnSpec, type ManagerCommandInfo, type ManagerModelInfo } from './acpAdapter.js';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';

export type { ManagerCommandInfo, ManagerModelInfo };

export interface ManagerBackendInfo {
  id: string;
  displayName: string;
  implemented: boolean;
}

interface ManagerAdapter extends ManagerBackendInfo {
  runTurn(
    gahProfile: string,
    input: {
      prompt: string;
      history: ChatTranscriptTurn[];
      onChunk: (text: string) => void;
      onToolResult: (name: string, text: string) => void;
    }
  ): Promise<{ reply: string; model: string | null; usage: ChatUsage | null }>;
  listCommands(gahProfile: string): Promise<ManagerCommandInfo[]>;
  listModels(gahProfile: string): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }>;
  setModel(gahProfile: string, modelId: string): Promise<void>;
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

  async listModels(): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }> {
    return { models: [], currentModelId: null };
  }

  async setModel(): Promise<void> {
    throw new Error(`${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}).`);
  }
}

function acpManagerAdapter(id: string, displayName: string, spawnSpec: () => import('./acpAdapter.js').SpawnSpec): ManagerAdapter {
  const backend = createAcpBackend(displayName, spawnSpec);
  return { id, displayName, implemented: true, ...backend };
}

const REGISTRY: Record<string, ManagerAdapter> = {
  hermes: acpManagerAdapter('hermes', 'Hermes', hermesSpawnSpec),
  codex: acpManagerAdapter('codex', 'Codex', codexSpawnSpec),
  claude: acpManagerAdapter('claude', 'Claude', claudeSpawnSpec),
  vibe: new NotImplementedAdapter('vibe', 'Vibe', 'no ACP or equivalent protocol available')
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
