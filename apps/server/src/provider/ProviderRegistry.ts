/**
 * Provider Registry - Tracks available providers and their status
 * Inspired by t3code's provider registry
 * 
 * After TICKET-113: Provider status and availability are now sourced from
 * the real gah CLI via gahCli.runStatus(). The old hardcoded defaults
 * have been replaced with dynamic data from the GAH backend.
 */

import { generateProviderInstanceId, getSupportedProviders } from '@git-agent-harness/shared';
import { runStatus, type StatusSnapshot } from '../gahCli.js';
import type { ProviderKind, ProviderStatus, ProviderInstanceId } from '@git-agent-harness/contracts';

interface CachedStatus {
  snapshot: StatusSnapshot;
  timestamp: number;
  ttl: number;
}

class ProviderRegistryImpl {
  private providerStatuses: Map<ProviderKind, ProviderStatus> = new Map();
  private providerVersions: Map<ProviderKind, string> = new Map();
  private availableProviders: Set<ProviderKind> = new Set();
  private cachedStatus: CachedStatus | null = null;
  private refreshPromise: Promise<void> | null = null;
  private defaultProfile: string = 'gah';
  
  constructor() {
    // Set default versions for known providers
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

    // Default every provider to available, not unavailable: `gah status`'s
    // availability list is a durable record of NEGATIVE observations only
    // (a backend that hit quota/auth failure gets an entry; a backend
    // that's simply never had a problem gets none at all -- see
    // src/availability.rs record_unavailable/record_available). Defaulting
    // absent-from-the-list to "unavailable" reads the lack of a bad record
    // as itself a bad sign, which is backwards -- it made Settings show
    // "unavailable" for backends real dispatches were actively using.
    for (const kind of getSupportedProviders()) {
      this.providerStatuses.set(kind, { type: 'available', version: this.providerVersions.get(kind) || '1.0.0' });
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
  
  /**
   * Set the default profile to use for status queries
   */
  setDefaultProfile(profile: string): void {
    this.defaultProfile = profile;
  }
  
  /**
   * Refresh all provider statuses from the GAH CLI
   */
  async refreshAllFromGah(): Promise<void> {
    // Avoid concurrent refreshes
    if (this.refreshPromise) {
      return this.refreshPromise;
    }
    
    this.refreshPromise = this.doRefreshAllFromGah();
    try {
      await this.refreshPromise;
    } finally {
      this.refreshPromise = null;
    }
  }
  
  /**
   * Internal method to refresh all from GAH CLI
   */
  private async doRefreshAllFromGah(): Promise<void> {
    try {
      // Get fresh status from gah CLI
      const snapshot = await runStatus(this.defaultProfile);
      this.cachedStatus = {
        snapshot,
        timestamp: Date.now(),
        ttl: 30000 // 30 second cache
      };
      
      // Update provider statuses from the availability data
      this.updateFromSnapshot(snapshot);
      
      console.log(`Refreshed provider statuses from GAH CLI (${snapshot.availability.length} backends)`);
      
    } catch (error) {
      console.error('Failed to refresh provider statuses from GAH CLI:', error);
      // Fall back to default behavior
      this.updateDefaultStatuses();
    }
  }
  
  /**
   * Update provider statuses from a GAH status snapshot
   */
  private updateFromSnapshot(snapshot: StatusSnapshot): void {
    // Reset available providers based on what GAH knows about
    this.availableProviders = new Set(getSupportedProviders());
    
    // Map availability entries to provider statuses
    for (const avail of snapshot.availability) {
      const kind = avail.backend as ProviderKind;
      
      // Skip if not a known provider kind
      if (!getSupportedProviders().includes(kind)) {
        continue;
      }
      
      if (avail.eligible_now) {
        this.providerStatuses.set(kind, { 
          type: 'available', 
          version: this.providerVersions.get(kind) || '1.0.0'
        });
        this.availableProviders.add(kind);
      } else {
        // Map unavailable reasons to status types
        let statusType: 'unavailable' | 'error' = 'unavailable';
        let errorMessage: string | undefined;
        
        if (avail.reason === 'quota_exhausted') {
          statusType = 'error';
          errorMessage = `Quota exhausted until ${avail.unavailable_until || 'unknown time'}`;
        } else if (avail.reason === 'auth_failure') {
          statusType = 'error';
          errorMessage = 'Authentication failed';
        } else if (avail.reason) {
          errorMessage = avail.reason;
        }
        
        if (statusType === 'error' && errorMessage) {
          this.providerStatuses.set(kind, { type: 'error', error: errorMessage });
        } else {
          this.providerStatuses.set(kind, { type: 'unavailable' });
        }
      }
    }
    
    // Ensure all supported providers have at least an unavailable status
    for (const kind of getSupportedProviders()) {
      if (!this.providerStatuses.has(kind)) {
        this.providerStatuses.set(kind, { type: 'unavailable' });
      }
    }
  }
  
  /**
   * Update to default statuses when GAH CLI is unavailable
   */
  private updateDefaultStatuses(): void {
    // Mark git providers as available (they don't depend on GAH backends)
    this.providerStatuses.set('github', { type: 'available', version: this.providerVersions.get('github') || '1.0.0' });
    this.providerStatuses.set('gitlab', { type: 'available', version: this.providerVersions.get('gitlab') || '1.0.0' });
    
    // Mark AI providers based on environment variables
    for (const kind of ['codex', 'claude', 'cursor', 'opencode', 'grok', 'openhands', 'agy', 'vibe']) {
      const envVar = this.getAuthEnvVar(kind as ProviderKind);
      const version = this.providerVersions.get(kind as ProviderKind) || '1.0.0';
      this.providerStatuses.set(
        kind as ProviderKind,
        process.env[envVar]
          ? { type: 'authenticated', version, userId: `user_${kind}` }
          : { type: 'available', version }
      );
    }
  }
  
  async refreshProviderStatus(kind: ProviderKind): Promise<ProviderStatus> {
    try {
      // Check if we have a cached status that's still fresh
      if (this.cachedStatus && (Date.now() - this.cachedStatus.timestamp) < this.cachedStatus.ttl) {
        return this.providerStatuses.get(kind) || { type: 'unavailable' };
      }
      
      // Refresh all statuses from GAH CLI
      await this.refreshAllFromGah();
      
      return this.providerStatuses.get(kind) || { type: 'unavailable' };
      
    } catch (error) {
      console.error(`Failed to refresh provider status for ${kind}:`, error);
      
      // Fall back to environment-based detection
      return this.getFallbackStatus(kind);
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
  
  /**
   * Get fallback status when GAH CLI is unavailable
   */
  private getFallbackStatus(kind: ProviderKind): ProviderStatus {
    const envVar = this.getAuthEnvVar(kind);
    
    switch (kind) {
      case 'github': {
        const version = this.providerVersions.get('github') || '1.0.0';
        return process.env.GITHUB_TOKEN
          ? { type: 'authenticated', version, userId: 'github-user' }
          : { type: 'available', version };
      }
      case 'gitlab': {
        const version = this.providerVersions.get('gitlab') || '1.0.0';
        return process.env.GITLAB_PAT
          ? { type: 'authenticated', version, userId: 'gitlab-user' }
          : { type: 'available', version };
      }
      case 'codex':
      case 'claude':
      case 'cursor':
      case 'opencode':
      case 'grok':
      case 'openhands':
      case 'agy':
      case 'vibe': {
        const version = this.providerVersions.get(kind) || '1.0.0';
        return process.env[envVar]
          ? { type: 'authenticated', version, userId: `user_${kind}` }
          : { type: 'available', version };
      }
      default:
        return { type: 'available', version: this.providerVersions.get(kind) || '1.0.0' };
    }
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