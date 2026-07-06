/**
 * Grok Provider Driver - STUB
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

export type GrokDriverEnv = ProviderDriverEnv & {
  GROK_API_KEY?: string;
  GROK_CLI_PATH?: string;
};

export type GrokCapabilities = {
  codeGeneration: boolean;
  chat: boolean;
  editing: boolean;
  commands: boolean;
  reasoning: boolean;
};

class GrokDriverImpl implements ProviderDriverInstance<GrokDriverEnv> {
  readonly kind: ProviderKind = 'grok';
  readonly version = '1.0.0';
  readonly capabilities: GrokCapabilities = {
    codeGeneration: true,
    chat: true,
    editing: true,
    commands: true,
    reasoning: true
  };
  
  constructor(private env: GrokDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('grok');
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('grok'),
        apiAvailable: !!this.env.GROK_API_KEY
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('grok');
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
    console.log(`[STUB] Starting Grok session with options:`, options);
    return sessionManager.startSession({
      providerKind: 'grok',
      instanceId: 'grok_instance_0',
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping Grok session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to Grok session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
}

export const GrokDriver: ProviderDriver<GrokDriverEnv> = {
  kind: 'grok',
  version: '1.0.0',
  create: (env: GrokDriverEnv) => new GrokDriverImpl(env),
  createSnapshot: () => new GrokDriverImpl({}).getSnapshot()
};