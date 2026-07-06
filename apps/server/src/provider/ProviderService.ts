/**
 * Provider Service - Main provider management service
 * Inspired by t3code's ProviderService but adapted for GAH
 */

import { getProviderRegistry } from './ProviderRegistry.js';
import { getSessionManager } from '../sessions/SessionManager.js';
import { getRustBackendProxy } from '../rustBackend.js';
import { getSupportedProviders, generateProviderInstanceId } from '@git-agent-harness/shared';
import type {
  ProviderKind,
  ProviderInstanceId,
  ProviderStatus,
  ServerProviderCatalog,
  ProviderInstance
} from '@git-agent-harness/contracts';

class ProviderServiceImpl {
  private registry: ReturnType<typeof getProviderRegistry>;
  private sessionManager: ReturnType<typeof getSessionManager>;
  
  constructor() {
    this.registry = getProviderRegistry();
    this.sessionManager = getSessionManager();
  }
  
  getProviderInstances(): ProviderInstance[] {
    return getSupportedProviders().map((kind, index) => ({
      instanceId: generateProviderInstanceId(kind, index),
      providerKind: kind,
      name: this.getProviderDisplayName(kind),
      isAvailable: this.registry.isProviderAvailable(kind),
      isAuthenticated: this.registry.isProviderAuthenticated(kind),
      version: this.registry.getProviderVersion(kind) || 'unknown'
    }));
  }
  
  getAllProviderStatuses(): Record<ProviderInstanceId, ProviderStatus> {
    const statuses: Record<ProviderInstanceId, ProviderStatus> = {};
    
    for (const kind of getSupportedProviders()) {
      const instanceId = generateProviderInstanceId(kind, 0);
      const status = this.registry.getProviderStatus(kind);
      statuses[instanceId] = status;
    }
    
    return statuses;
  }
  
  async refreshProvider(instanceId: ProviderInstanceId): Promise<ProviderStatus> {
    const providerKind = this.extractProviderKindFromInstanceId(instanceId);
    const status = await this.registry.refreshProviderStatus(providerKind);
    return status;
  }
  
  getProviderDisplayName(kind: ProviderKind): string {
    const displayNames: Record<ProviderKind, string> = {
      github: 'GitHub',
      gitlab: 'GitLab',
      codex: 'Codex',
      claude: 'Claude',
      cursor: 'Cursor',
      opencode: 'OpenCode',
      grok: 'Grok',
      openhands: 'OpenHands',
      agy: 'AGY',
      vibe: 'Vibe',
      auto: 'Auto'
    };
    return displayNames[kind] || kind;
  }
  
  getProviderDescription(kind: ProviderKind): string {
    const descriptions: Record<ProviderKind, string> = {
      github: 'GitHub repository hosting and CI/CD',
      gitlab: 'GitLab repository hosting and DevOps',
      codex: 'OpenAI Codex coding assistant',
      claude: 'Anthropic Claude coding assistant',
      cursor: 'Cursor AI coding assistant',
      opencode: 'OpenCode AI coding assistant',
      grok: 'xAI Grok coding assistant',
      openhands: 'OpenHands agent framework',
      agy: 'AGY coding agent',
      vibe: 'Mistral Vibe CLI agent',
      auto: 'Auto backend selection'
    };
    return descriptions[kind] || `Provider: ${kind}`;
  }
  
  async startSessionForProvider(providerKind: ProviderKind, options: {
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
  }): Promise<string> {
    const sessionManager = getSessionManager();
    const instanceId = generateProviderInstanceId(providerKind, 0);
    
    const session = await sessionManager.startSession({
      providerKind,
      instanceId,
      ...options
    });
    
    return session.id;
  }
  
  async dispatchToProvider(providerKind: ProviderKind, options: {
    profile: string;
    mode: string;
    backend?: string;
    target?: string;
    branch?: string;
    mr?: string;
    budget?: number;
    model?: string;
    dryRun?: boolean;
    allowDraftFail?: boolean;
    prod?: boolean;
    currentBranch?: boolean;
    retries?: number;
    allowUnknownRedBaseline?: boolean;
    escalate?: boolean;
  }): Promise<{ success: boolean; sessionId?: string; error?: string }> {
    try {
      const rustBackend = getRustBackendProxy();
      
      // If Rust backend is available, use it
      if (rustBackend.isBackendReady()) {
        // For now, simulate dispatch via session manager
        const sessionManager = getSessionManager();
        const session = await sessionManager.startSession({
          providerKind,
          instanceId: generateProviderInstanceId(providerKind, 0),
          repo: options.target || '',
          branch: options.branch,
          target: options.target,
          mode: options.mode,
          backend: options.backend,
          model: options.model,
          budget: options.budget
        });
        
        return { success: true, sessionId: session.id };
      } else {
        // Fallback to TypeScript-only mode
        console.warn('Rust backend not available, running dispatch in TypeScript mode');
        
        const sessionManager = getSessionManager();
        const session = await sessionManager.startSession({
          providerKind,
          instanceId: generateProviderInstanceId(providerKind, 0),
          repo: options.target || '',
          branch: options.branch,
          target: options.target,
          mode: options.mode,
          backend: options.backend,
          model: options.model,
          budget: options.budget
        });
        
        return { success: true, sessionId: session.id };
      }
    } catch (error) {
      return { 
        success: false, 
        error: error instanceof Error ? error.message : String(error) 
      };
    }
  }
  
  private extractProviderKindFromInstanceId(instanceId: ProviderInstanceId): ProviderKind {
    // Extract kind from instanceId like "github_instance_0"
    const match = instanceId.match(/^([^_]+)_/);
    return match ? match[1] as ProviderKind : 'github';
  }
}

const providerService = new ProviderServiceImpl();

export function getProviderService(): ProviderServiceImpl {
  return providerService;
}

export function createProviderService() {
  return new ProviderServiceImpl();
}

// Re-export provider catalog type
export type { ServerProviderCatalog };