/**
 * Status Aggregator - Fetches and aggregates status from local and remote hosts
 * Part of MS-2: Remote gah status aggregation across hosts
 */

import { getHostRegistry, type HostId } from './HostRegistry.js';
import { runStatus, type StatusSnapshot } from '../gahCli.js';
import type { ServerMessage } from '@git-agent-harness/contracts';

interface HostStatusResult {
  ok: boolean;
  host_id: HostId;
  snapshot?: StatusSnapshot;
  error?: string;
  fetched_at: string;
}

class StatusAggregatorImpl {
  private cache: Map<HostId, HostStatusResult> = new Map();
  private refreshPromise: Promise<void> | null = null;
  private cacheTTL: number = 30000; // 30 seconds
  
  constructor(private localProfile: string = 'gah') {}
  
  /**
   * Get merged status from local host and all configured remote hosts
   */
  async getMergedStatus(): Promise<Record<HostId, HostStatusResult>> {
    const hostRegistry = getHostRegistry();
    const allHosts = hostRegistry.getAllHosts();
    const result: Record<HostId, HostStatusResult> = {};
    
    // Always include local host
    const localHostId = 'local';
    result[localHostId] = await this.getHostStatus(localHostId, this.localProfile);
    
    // Add all remote hosts
    for (const host of allHosts) {
      result[host.id] = await this.getHostStatus(host.id, host.profile || this.localProfile);
    }
    
    return result;
  }
  
  /**
   * Get status for a specific host
   */
  private async getHostStatus(hostId: HostId, profile: string): Promise<HostStatusResult> {
    // Check cache first
    const cached = this.cache.get(hostId);
    if (cached && (Date.now() - new Date(cached.fetched_at).getTime()) < this.cacheTTL) {
      return cached;
    }
    
    try {
      if (hostId === 'local') {
        // Local host - use gah CLI directly
        const snapshot = await runStatus(profile);
        const result = {
          ok: true,
          host_id: hostId,
          snapshot,
          fetched_at: new Date().toISOString()
        };
        this.cache.set(hostId, result);
        return result;
      } else {
        // Remote host - fetch via HTTP
        const hostRegistry = getHostRegistry();
        const hostConfig = hostRegistry.getHostConfig(hostId);
        
        if (!hostConfig) {
          return {
            ok: false,
            host_id: hostId,
            error: 'Host configuration not found',
            fetched_at: new Date().toISOString()
          };
        }
        
        const url = new URL('/api/status', hostConfig.base_url);
        if (hostConfig.profile) {
          url.searchParams.append('profile', hostConfig.profile);
        }
        
        const headers: Record<string, string> = {
          'Content-Type': 'application/json'
        };
        
        if (hostConfig.auth_token) {
          headers['Authorization'] = `Bearer ${hostConfig.auth_token}`;
        }
        
        const response = await fetch(url.toString(), {
          method: 'GET',
          headers
        });
        
        if (!response.ok) {
          return {
            ok: false,
            host_id: hostId,
            error: `HTTP ${response.status}: ${response.statusText}`,
            fetched_at: new Date().toISOString()
          };
        }
        
        const snapshot = await response.json() as StatusSnapshot;
        const result = {
          ok: true,
          host_id: hostId,
          snapshot,
          fetched_at: new Date().toISOString()
        };
        this.cache.set(hostId, result);
        return result;
      }
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      const result = {
        ok: false,
        host_id: hostId,
        error: errorMessage,
        fetched_at: new Date().toISOString()
      };
      this.cache.set(hostId, result);
      return result;
    }
  }
  
  /**
   * Refresh all host statuses
   */
  async refreshAll(): Promise<void> {
    // Avoid concurrent refreshes
    if (this.refreshPromise) {
      return this.refreshPromise;
    }
    
    this.refreshPromise = this.doRefreshAll();
    try {
      await this.refreshPromise;
    } finally {
      this.refreshPromise = null;
    }
  }
  
  /**
   * Internal method to refresh all host statuses
   */
  private async doRefreshAll(): Promise<void> {
    const hostRegistry = getHostRegistry();
    const allHosts = hostRegistry.getAllHosts();
    
    // Refresh local host
    await this.getHostStatus('local', this.localProfile);
    
    // Refresh all remote hosts in parallel
    const refreshPromises = allHosts.map(host => 
      this.getHostStatus(host.id, host.profile || this.localProfile)
    );
    
    await Promise.all(refreshPromises);
    
    console.log(`Refreshed status for ${allHosts.length + 1} host(s)`);
  }
  
  /**
   * Set the local profile to use for status queries
   */
  setLocalProfile(profile: string): void {
    this.localProfile = profile;
  }
  
  /**
   * Clear the cache
   */
  clearCache(): void {
    this.cache.clear();
  }
  
  /**
   * Set cache TTL in milliseconds
   */
  setCacheTTL(ttl: number): void {
    this.cacheTTL = ttl;
  }
}

const statusAggregator = new StatusAggregatorImpl();

export function getStatusAggregator(): StatusAggregatorImpl {
  return statusAggregator;
}

export function createStatusAggregator(profile?: string): StatusAggregatorImpl {
  return new StatusAggregatorImpl(profile);
}

/**
 * Create a server.hostsStatus message for WebSocket broadcast
 */
export function createHostsStatusMessage(hostsStatus: Record<HostId, HostStatusResult>): ServerMessage {
  return {
    type: 'server.hostsStatus',
    hostsStatus,
    timestamp: Date.now()
  };
}