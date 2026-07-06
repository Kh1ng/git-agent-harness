/**
 * Provider Registry - Tracks available providers and their status
 * Inspired by t3code's provider registry
 * Updated per TICKET-113 to use GAH CLI for real availability data
 */

import { generateProviderInstanceId, getSupportedProviders } from '@git-agent-harness/shared';
import type { ProviderKind, ProviderStatus, ProviderInstanceId } from '@git-agent-harness/contracts';
import { runStatus, getAvailabilityFromStatus, isBackendAvailable, type StatusSnapshot, type ScopeStatusJson } from '../gahCli.js';

// Cache for status snapshots
let statusCache: Map<string, { snapshot: StatusSnapshot; timestamp: number }> = new Map();
const STATUS_CACHE_TTL = 30000; // 30 seconds

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
    
    // Initialize with default versions
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
    
    // Start with unavailable status for all, will be updated from CLI
    for (const kind of this.availableProviders) {
      this.providerStatuses.set(kind, { type: 'unavailable' });
    }
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
      // [TICKET-113] Use GAH CLI to get real availability status
      // For now, we'll use the provider kind as the profile name
      // In a real implementation, this would be configurable
      const profile = kind;
      
      try {
        const snapshot = await this.getStatusSnapshot(profile);
        const availabilityMap = getAvailabilityFromStatus(snapshot);
        
        // Check if this backend is available
        const isAvailable = isBackendAvailable(snapshot, kind);
        
        if (isAvailable) {
          // Find the specific scope for this backend
          const scope = snapshot.availability.find(s => s.backend === kind);
          const version = scope?.model ? `${kind}:${scope.model}` : this.providerVersions.get(kind) || '1.0.0';
          const userId = scope?.model ? `user_${kind}` : undefined;
          
          this.providerStatuses.set(kind, { 
            type: 'available', 
            version,
            userId
          });
        } else {
          // Check if the backend is temporarily unavailable
          const scope = snapshot.availability.find(s => s.backend === kind);
          if (scope) {
            // Backend is known but not currently eligible
            this.providerStatuses.set(kind, { 
              type: 'unavailable',
              version: this.providerVersions.get(kind) || '1.0.0'
            });
          } else {
            // Backend not found in availability list
            this.providerStatuses.set(kind, { type: 'unavailable' });
          }
        }
        
        return this.providerStatuses.get(kind)!;
        
      } catch (cliError) {
        console.warn(`[TICKET-113] Failed to get CLI status for ${kind}, falling back to env detection:`, cliError);
        
        // Fall back to environment variable detection for AI providers
        if (['codex', 'claude', 'cursor', 'opencode', 'grok', 'openhands', 'agy', 'vibe'].includes(kind)) {
          const envVar = this.getAuthEnvVar(kind as any);
          this.providerStatuses.set(kind as any, {
            type: process.env[envVar] ? 'authenticated' : 'unavailable',
            version: this.providerVersions.get(kind as any) || '1.0.0',
            userId: process.env[envVar] ? `user_${kind}` : undefined
          });
        } else {
          // For github/gitlab, assume unavailable if CLI check failed
          this.providerStatuses.set(kind as any, { type: 'unavailable' });
        }
        
        return this.providerStatuses.get(kind as any)!;
      }
      
    } catch (error) {
      console.error(`[TICKET-113] Failed to refresh provider status for ${kind}:`, error);
      return { type: 'error', error: String(error) };
    }
  }
  
  /**
   * Get or fetch status snapshot from GAH CLI with caching
   */
  private async getStatusSnapshot(profile: string): Promise<StatusSnapshot> {
    // Check cache
    const cached = statusCache.get(profile);
    if (cached && Date.now() - cached.timestamp < STATUS_CACHE_TTL) {
      return cached.snapshot;
    }
    
    // Fetch fresh snapshot
    const snapshot = await runStatus(profile);
    statusCache.set(profile, { snapshot, timestamp: Date.now() });
    return snapshot;
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