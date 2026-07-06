/**
 * Claude Provider Driver - STUB
 * This is a stub implementation for Claude provider  
 * Will be fully implemented to match t3code's ClaudeDriver
 */

import type { ProviderDriver, ProviderDriverEnv, ProviderDriverInstance } from '../ProviderDriver.js';
import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';
import { getProviderRegistry } from '../ProviderRegistry.js';
import { getSessionManager } from '../../sessions/SessionManager.js';

// Claude-specific environment requirements
export type ClaudeDriverEnv = ProviderDriverEnv & {
  ANTHROPIC_API_KEY?: string;
  CLAUDE_CLI_PATH?: string;
};

// Claude provider capabilities
export type ClaudeCapabilities = {
  codeGeneration: boolean;
  chat: boolean;
  editing: boolean;
  commands: boolean;
  tools: boolean;
};

class ClaudeDriverImpl implements ProviderDriverInstance<ClaudeDriverEnv> {
  readonly kind: ProviderKind = 'claude';
  readonly version = '1.0.0';
  readonly capabilities: ClaudeCapabilities = {
    codeGeneration: true,
    chat: true,
    editing: true,
    commands: true,
    tools: true
  };
  
  constructor(private env: ClaudeDriverEnv) {}
  
  async getSnapshot(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    const status = registry.getProviderStatus('claude');
    
    return {
      kind: this.kind,
      version: this.version,
      status,
      capabilities: this.capabilities,
      metadata: {
        isAuthenticated: registry.isProviderAuthenticated('claude'),
        apiAvailable: this.isClaudeApiAvailable()
      }
    };
  }
  
  async refresh(): Promise<ServerProvider> {
    const registry = getProviderRegistry();
    await registry.refreshProviderStatus('claude');
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
    const instanceId = 'claude_instance_0';
    
    console.log(`[STUB] Starting Claude session with options:`, options);
    
    return sessionManager.startSession({
      providerKind: 'claude',
      instanceId,
      ...options
    });
  }
  
  async stopSession(sessionId: string): Promise<Session> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Stopping Claude session ${sessionId}`);
    return sessionManager.stopSession(sessionId);
  }
  
  async sendCommand(sessionId: string, command: string): Promise<void> {
    const sessionManager = getSessionManager();
    console.log(`[STUB] Sending command to Claude session ${sessionId}: ${command}`);
    await sessionManager.sendCommand(sessionId, command);
  }
  
  private isClaudeApiAvailable(): boolean {
    return !!(this.env.ANTHROPIC_API_KEY || process.env.ANTHROPIC_API_KEY);
  }
}

// Export the driver and its factory
export const ClaudeDriver: ProviderDriver<ClaudeDriverEnv> = {
  kind: 'claude',
  version: '1.0.0',
  create: (env: ClaudeDriverEnv) => new ClaudeDriverImpl(env),
  createSnapshot: () => new ClaudeDriverImpl({}).getSnapshot()
};