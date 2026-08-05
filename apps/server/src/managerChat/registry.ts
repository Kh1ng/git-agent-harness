/**
 * Manager backend registry for the manager-chat MVP.
 *
 * Only Hermes has a real adapter today. Codex/Claude get real adapters
 * under #820/#821; this registry exists now so the UI can offer backend
 * selection without waiting on those -- picking an unimplemented backend
 * fails loudly with a clear message instead of the chat silently doing
 * nothing or falling back to a different backend than the one selected.
 */

import { runHermesTurn } from './hermesAdapter.js';

export interface ManagerTurnResult {
  sessionId: string;
  reply: string;
}

export interface ManagerBackendInfo {
  id: string;
  displayName: string;
  implemented: boolean;
}

interface ManagerAdapter extends ManagerBackendInfo {
  runTurn(message: string, resumeSessionId: string | undefined): Promise<ManagerTurnResult>;
}

class NotImplementedAdapter implements ManagerAdapter {
  implemented = false;
  constructor(
    public id: string,
    public displayName: string,
    private trackingIssue: string
  ) {}

  async runTurn(): Promise<ManagerTurnResult> {
    throw new Error(
      `${this.displayName} isn't wired up as a manager chat backend yet (${this.trackingIssue}). Pick a different backend in Settings.`
    );
  }
}

const REGISTRY: Record<string, ManagerAdapter> = {
  hermes: {
    id: 'hermes',
    displayName: 'Hermes',
    implemented: true,
    runTurn: runHermesTurn
  },
  codex: new NotImplementedAdapter('codex', 'Codex', 'issue #820'),
  claude: new NotImplementedAdapter('claude', 'Claude', 'issue #821'),
  vibe: new NotImplementedAdapter('vibe', 'Vibe', 'no tracking issue yet')
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
