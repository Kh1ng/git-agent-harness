/**
 * Codex Provider Driver - STUB
 * This is a stub implementation for Codex provider
 * Will be fully implemented to match t3code's CodexDriver
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

// Codex-specific environment requirements
export type CodexDriverEnv = ProviderDriverEnv & {
  CODEX_API_KEY?: string;
  CODEX_CLI_PATH?: string;
};

// Codex provider capabilities
export type CodexCapabilities = {
  codeGeneration: boolean;
  chat: boolean;
  editing: boolean;
  commands: boolean;
};

class CodexDriverImpl implements ProviderDriverInstance<CodexDriverEnv> {
  readonly kind: ProviderKind = 'codex';
  readonly version = '1.0.0';
  readonly capabilities: CodexCapabilities = {
    codeGeneration: true,
    chat: true,
    editing: true,
    commands: true
  };
  
  constructor(private env: CodexDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('codex');
    
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('codex'),
        apiAvailable: this.isCodexApiAvailable()
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('codex');
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
    const instanceId = 'codex_instance_0';
    
    console.log(`[STUB] Starting Codex session with options:`, options);
    
    return sessionManager.startSession({
      providerKind: 'codex',
      instanceId,
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping Codex session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to Codex session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
  
  private isCodexApiAvailable(): boolean {
    return !!(this.env.CODEX_API_KEY || process.env.CODEX_API_KEY);
  }
}

// Export the driver and its factory
export const CodexDriver: ProviderDriver<CodexDriverEnv> = {
  kind: 'codex',
  version: '1.0.0',
  create: (env: CodexDriverEnv) => new CodexDriverImpl(env),
  createSnapshot: () => new CodexDriverImpl({}).getSnapshot()
};