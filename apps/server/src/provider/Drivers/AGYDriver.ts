/**
 * AGY Provider Driver - STUB
 */

import type { ProviderDriver, ProviderDriverEnv } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

export type AGYDriverEnv = ProviderDriverEnv & {
  AGY_API_KEY?: string;
  AGY_CLI_PATH?: string;
};

export type AGYCapabilities = {
  codeGeneration: boolean;
  multiAgent: boolean;
  taskDecomposition: boolean;
  resourceAccess: boolean;
};

class AGYDriverImpl implements ProviderDriverInstance<AGYDriverEnv> {
  readonly kind: ProviderKind = 'agy';
  readonly version = '1.0.0';
  readonly capabilities: AGYCapabilities = {
    codeGeneration: true,
    multiAgent: true,
    taskDecomposition: true,
    resourceAccess: true
  };
  
  constructor(private env: AGYDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('agy');
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('agy'),
        apiAvailable: !!this.env.AGY_API_KEY
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('agy');
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
    console.log(`[STUB] Starting AGY session with options:`, options);
    return sessionManager.startSession({
      providerKind: 'agy',
      instanceId: 'agy_instance_0',
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping AGY session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to AGY session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
}

export const AGYDriver: ProviderDriver<AGYDriverEnv> = {
  kind: 'agy',
  version: '1.0.0',
  create: (env: AGYDriverEnv) => new AGYDriverImpl(env),
  createSnapshot: () => new AGYDriverImpl({}).getSnapshot()
};