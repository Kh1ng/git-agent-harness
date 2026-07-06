/**
 * Vibe Provider Driver - STUB
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

export type VibeDriverEnv = ProviderDriverEnv & {
  VIBE_API_KEY?: string;
  VIBE_CLI_PATH?: string;
};

export type VibeCapabilities = {
  codeGeneration: boolean;
  cliIntegration: boolean;
  worktreeManagement: boolean;
  multiBackend: boolean;
};

class VibeDriverImpl implements ProviderDriverInstance<VibeDriverEnv> {
  readonly kind: ProviderKind = 'vibe';
  readonly version = '1.0.0';
  readonly capabilities: VibeCapabilities = {
    codeGeneration: true,
    cliIntegration: true,
    worktreeManagement: true,
    multiBackend: true
  };
  
  constructor(private env: VibeDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('vibe');
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('vibe'),
        apiAvailable: !!this.env.VIBE_API_KEY
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('vibe');
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
    console.log(`[STUB] Starting Vibe session with options:`, options);
    return sessionManager.startSession({
      providerKind: 'vibe',
      instanceId: 'vibe_instance_0',
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping Vibe session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to Vibe session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
}

export const VibeDriver: ProviderDriver<VibeDriverEnv> = {
  kind: 'vibe',
  version: '1.0.0',
  create: (env: VibeDriverEnv) => new VibeDriverImpl(env),
  createSnapshot: () => new VibeDriverImpl({}).getSnapshot()
};