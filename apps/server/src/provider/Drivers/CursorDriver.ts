/**
 * Cursor Provider Driver - STUB
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

export type CursorDriverEnv = ProviderDriverEnv & {
  CURSOR_API_KEY?: string;
  CURSOR_CLI_PATH?: string;
};

export type CursorCapabilities = {
  codeGeneration: boolean;
  chat: boolean;
  editing: boolean;
  commands: boolean;
  ideIntegration: boolean;
};

class CursorDriverImpl implements ProviderDriverInstance<CursorDriverEnv> {
  readonly kind: ProviderKind = 'cursor';
  readonly version = '1.0.0';
  readonly capabilities: CursorCapabilities = {
    codeGeneration: true,
    chat: true,
    editing: true,
    commands: true,
    ideIntegration: true
  };
  
  constructor(private env: CursorDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('cursor');
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('cursor'),
        apiAvailable: !!this.env.CURSOR_API_KEY
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('cursor');
    return this.getSnapshot();
  }
  
  async startSession(options: {
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
  }): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Starting Cursor session with options:`, options);
    return sessionManager.startSession({
      providerKind: 'cursor',
      instanceId: 'cursor_instance_0',
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping Cursor session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to Cursor session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
}

export const CursorDriver: ProviderDriver<CursorDriverEnv> = {
  kind: 'cursor',
  version: '1.0.0',
  create: (env: CursorDriverEnv) => new CursorDriverImpl(env),
  createSnapshot: () => new CursorDriverImpl({}).getSnapshot()
};