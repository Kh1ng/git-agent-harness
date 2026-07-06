/**
 * OpenCode Provider Driver - STUB
 */

import type { ProviderDriver, ProviderDriverEnv } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

export type OpenCodeDriverEnv = ProviderDriverEnv & {
  OPENCODE_API_KEY?: string;
  OPENCODE_CLI_PATH?: string;
};

export type OpenCodeCapabilities = {
  codeGeneration: boolean;
  chat: boolean;
  editing: boolean;
  commands: boolean;
  multiModel: boolean;
};

class OpenCodeDriverImpl implements ProviderDriverInstance<OpenCodeDriverEnv> {
  readonly kind: ProviderKind = 'opencode';
  readonly version = '1.0.0';
  readonly capabilities: OpenCodeCapabilities = {
    codeGeneration: true,
    chat: true,
    editing: true,
    commands: true,
    multiModel: true
  };
  
  constructor(private env: OpenCodeDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('opencode');
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('opencode'),
        apiAvailable: !!this.env.OPENCODE_API_KEY
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('opencode');
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
    console.log(`[STUB] Starting OpenCode session with options:`, options);
    return sessionManager.startSession({
      providerKind: 'opencode',
      instanceId: 'opencode_instance_0',
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping OpenCode session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to OpenCode session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
}

export const OpenCodeDriver: ProviderDriver<OpenCodeDriverEnv> = {
  kind: 'opencode',
  version: '1.0.0',
  create: (env: OpenCodeDriverEnv) => new OpenCodeDriverImpl(env),
  createSnapshot: () => new OpenCodeDriverImpl({}).getSnapshot()
};