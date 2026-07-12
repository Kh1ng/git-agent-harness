/**
 * Host Registry - Tracks configured remote GAH hosts for status aggregation
 * Part of MS-2: Remote gah status aggregation across hosts
 */

import type { StatusSnapshot } from '@git-agent-harness/contracts';

export type HostId = string;

export interface HostConfig {
  id: HostId;
  base_url: string;
  auth_token?: string;
  profile?: string;
}

class HostRegistryImpl {
  private hosts: Map<HostId, HostConfig> = new Map();
  
  constructor() {
    // Initialize with environment variable configuration
    this.loadFromEnvironment();
  }
  
  private loadFromEnvironment(): void {
    // Look for GAH_REMOTE_HOSTS environment variable
    // Format: host1:http://host1:3773|host2:http://host2:3773[|auth_token1|auth_token2]
    const remoteHosts = process.env.GAH_REMOTE_HOSTS;
    if (!remoteHosts) {
      return;
    }
    
    try {
      const hostEntries = remoteHosts.split('|');
      for (const entry of hostEntries) {
        if (!entry.trim()) continue;
        
        const parts = entry.split(':');
        if (parts.length < 2) {
          console.warn(`Invalid host entry format: ${entry}`);
          continue;
        }
        
        const id = parts[0];
        const base_url = parts[1];
        let auth_token: string | undefined = undefined;
        let profile: string | undefined = undefined;
        
        // Check for optional auth token and profile
        if (parts.length >= 3 && !parts[2].startsWith('http')) {
          auth_token = parts[2];
        }
        if (parts.length >= 4) {
          profile = parts[3];
        }
        
        this.hosts.set(id, { id, base_url, auth_token, profile });
      }
      
      console.log(`Loaded ${this.hosts.size} remote host(s) from GAH_REMOTE_HOSTS`);
    } catch (error) {
      console.error('Failed to parse GAH_REMOTE_HOSTS:', error);
    }
  }
  
  /**
   * Get all configured host IDs
   */
  getHostIds(): HostId[] {
    return Array.from(this.hosts.keys());
  }
  
  /**
   * Get host configuration by ID
   */
  getHostConfig(hostId: HostId): HostConfig | undefined {
    return this.hosts.get(hostId);
  }
  
  /**
   * Get all host configurations
   */
  getAllHosts(): HostConfig[] {
    return Array.from(this.hosts.values());
  }
  
  /**
   * Check if a host is configured
   */
  hasHost(hostId: HostId): boolean {
    return this.hosts.has(hostId);
  }
  
  /**
   * Add or update a host configuration
   */
  setHost(config: HostConfig): void {
    this.hosts.set(config.id, config);
  }
  
  /**
   * Remove a host configuration
   */
  removeHost(hostId: HostId): boolean {
    return this.hosts.delete(hostId);
  }
  
  /**
   * Clear all host configurations
   */
  clear(): void {
    this.hosts.clear();
  }
}

const hostRegistry = new HostRegistryImpl();

export function getHostRegistry(): HostRegistryImpl {
  return hostRegistry;
}

export function createHostRegistry(): HostRegistryImpl {
  return new HostRegistryImpl();
}