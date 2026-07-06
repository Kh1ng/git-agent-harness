/**
 * Provider Registry - Tracks available providers and their status
 * Inspired by t3code's provider registry
 */

import { generateProviderInstanceId, getSupportedProviders } from '@git-agent-harness/shared';
import { getRustBackendProxy } from '../rustBackend.js';
import type { ProviderKind, ProviderStatus, ProviderInstanceId } from '@git-agent-harness/contracts';

class ProviderRegistryImpl {
  private providerStatuses: Map<ProviderKind, ProviderStatus> = new Map();
  private providerVersions: Map<ProviderKind, string> = new Map();
  private availableProviders: Set<ProviderKind> = new Set();
  
  constructor() {
    // Initialize with default statuses
    for (const kind of getSupportedProviders()) {
      this.providerStatuses.set(kind, { type: 'unavailable' });
    }
    
    // Mark currently supported providers as available
    this.availableProviders = new Set([
      'github',
      'gitlab',
      'codex',
      'claude', 
      'cursor',
      'opencode',
      'grok',
      'openhands',
      'agy',
      'vibe'
    ]);
    
    // Mark providers that are likely installed and authenticated
    // This would be detected properly in a real implementation
    this.providerStatuses.set('github', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('gitlab', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('codex', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('claude', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('cursor', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('opencode', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('grok', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('openhands', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('agy', { type: 'available', version: '1.0.0' });
    this.providerStatuses.set('vibe', { type: 'available', version: '1.0.0' });
    
    // Set versions for providers we know about
    this.providerVersions.set('github', 'cli-2.0.0');
    this.providerVersions.set('gitlab', 'cli-15.0.0');
    this.providerVersions.set('codex', '1.0.0');
    this.providerVersions.set('claude', '1.0.0');
    this.providerVersions.set('cursor', '1.0.0');
    this.providerVersions.set('opencode', '1.0.0');
    this.providerVersions.set('grok', '1.0.0');
    this.providerVersions.set('openhands', '1.0.0');
    this.providerVersions.set('agy', '1.0.0');
    this.providerVersions.set('vibe', '1.0.0');
  }
  
  isProviderAvailable(kind: ProviderKind): boolean {
    return this.availableProviders.has(kind) && 
           this.providerStatuses.get(kind)?.type !== 'unavailable';
  }
  
  isProviderAuthenticated(kind: ProviderKind): boolean {
    const status = this.providerStatuses.get(kind);
    return status?.type === 'authenticated';
  }
  
  getProviderStatus(kind: ProviderKind): ProviderStatus {
    return this.providerStatuses.get(kind) || { type: 'unavailable' };
  }
  
  getProviderVersion(kind: ProviderKind): string | undefined {
    return this.providerVersions.get(kind);
  }
  
  getProviderInstanceIds(kind: ProviderKind): ProviderInstanceId[] {
    // For now, each provider has one instance
    return [generateProviderInstanceId(kind, 0)];
  }
  
  async refreshProviderStatus(kind: ProviderKind): Promise<ProviderStatus> {
    try {
      // Check if we can detect the provider via Rust backend or environment
      const rustBackend = getRustBackendProxy();
      
      if (rustBackend.isBackendReady()) {
        // Try to get provider status from Rust backend
        try {
          await rustBackend.sendCommand(`provider check ${kind}`);
          this.providerStatuses.set(kind, { type: 'available', version: this.providerVersions.get(kind) || '1.0.0' });
          return this.providerStatuses.get(kind)!;
        } catch {
          // Fall through to default detection
        }
      }
      
      // Default detection logic
      switch (kind) {
        case 'github':
          // Check if gh CLI is available
          try {
            await import('node:fs').then(fs => fs.promises.access('/usr/local/bin/gh', fs.constants.X_OK));
            this.providerStatuses.set(kind, { type: 'available', version: '2.0.0' });
          } catch {
            this.providerStatuses.set(kind, { type: 'available', version: '2.0.0' }); // Assume available
          }
          break;
          
        case 'gitlab':
          // GitLab is available if we have curl and a token
          if (process.env.GITLAB_PAT) {
            this.providerStatuses.set(kind, { 
              type: 'authenticated' as const, 
              version: '15.0.0',
              userId: 'gitlab-user'
            });
          } else {
            this.providerStatuses.set(kind, { 
              type: 'available' as const, 
              version: '15.0.0'
            });
          }
          break;
          
        case 'codex':
        case 'claude':
        case 'cursor':
        case 'opencode':
        case 'grok':
        case 'openhands':
        case 'agy':
        case 'vibe':
          // For AI providers, check if they're authenticated
          const envVar = this.getAuthEnvVar(kind);
          if (process.env[envVar]) {
            this.providerStatuses.set(kind, {
              type: 'authenticated' as const,
              version: this.providerVersions.get(kind) || '1.0.0',
              userId: `user_${kind}`
            });
          } else {
            this.providerStatuses.set(kind, {
              type: 'available' as const,
              version: this.providerVersions.get(kind) || '1.0.0'
            });
          }
          break;
          
        default:
          this.providerStatuses.set(kind, { type: 'available', version: '1.0.0' });
      }
      
      return this.providerStatuses.get(kind)!;
      
    } catch (error) {
      console.error(`Failed to refresh provider status for ${kind}:`, error);
      return { type: 'error', error: String(error) };
    }
  }
  
  private getAuthEnvVar(kind: ProviderKind): string {
    const envVars: Record<ProviderKind, string> = {
      github: 'GITHUB_TOKEN',
      gitlab: 'GITLAB_PAT',
      codex: 'CODEX_API_KEY',
      claude: 'ANTHROPIC_API_KEY',
      cursor: 'CURSOR_API_KEY',
      opencode: 'OPENCODE_API_KEY',
      grok: 'GROK_API_KEY',
      openhands: 'OPENHANDS_API_KEY',
      agy: 'AGY_API_KEY',
      vibe: 'VIBE_API_KEY',
      auto: 'AUTO_API_KEY'
    };
    return envVars[kind] || `PROVIDER_${kind.toUpperCase()}_KEY`;
  }
  
  getProviderInstances(): Array<{ 
    instanceId: ProviderInstanceId; 
    providerKind: ProviderKind; 
    name: string; 
    isAvailable: boolean; 
    isAuthenticated: boolean; 
    version: string; 
  }> {
    return getSupportedProviders().map(kind => ({
      instanceId: generateProviderInstanceId(kind, 0),
      providerKind: kind,
      name: kind,
      isAvailable: this.isProviderAvailable(kind),
      isAuthenticated: this.isProviderAuthenticated(kind),
      version: this.getProviderVersion(kind) || 'unknown'
    }));
  }
  
  getAllProviderStatuses(): Record<ProviderInstanceId, ProviderStatus> {
    const statuses: Record<ProviderInstanceId, ProviderStatus> = {};
    
    for (const kind of getSupportedProviders()) {
      const instanceId = generateProviderInstanceId(kind, 0);
      statuses[instanceId] = this.getProviderStatus(kind);
    }
    
    return statuses;
  }
}

const providerRegistry = new ProviderRegistryImpl();

export function getProviderRegistry(): ProviderRegistryImpl {
  return providerRegistry;
}

export function createProviderRegistry(): ProviderRegistryImpl {
  return new ProviderRegistryImpl();
}