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
    // Format: host1:http://host1:3773|host2:http://host2:3773|auth_token1|auth_token2|profile1|profile2
    const remoteHosts = process.env.GAH_REMOTE_HOSTS;
    if (!remoteHosts) {
      return;
    }
    
    try {
      const hostEntries = remoteHosts.split('|');
      if (hostEntries.length === 0) {
        return;
      }
      
      // First pass: count how many host definitions we have (entries with colon and URL)
      const hosts: { id: string; base_url: string }[] = [];
      for (const entry of hostEntries) {
        if (!entry.trim()) continue;
        
        // Split only on the first colon to handle URLs with colons
        const colonIndex = entry.indexOf(':');
        if (colonIndex === -1) {
          continue;
        }
        
        const id = entry.substring(0, colonIndex);
        const base_url = entry.substring(colonIndex + 1);
        
        if (base_url.startsWith('http://') || base_url.startsWith('https://')) {
          hosts.push({ id, base_url });
        }
      }
      
      if (hosts.length === 0) {
        console.warn('No valid host definitions found in GAH_REMOTE_HOSTS');
        return;
      }
      
      // Now parse auth tokens and profiles
      // Auth tokens and profiles come after host definitions in order
      const authTokens: (string | undefined)[] = [];
      const profiles: (string | undefined)[] = [];
      
      // Count how many host definitions we have
      const numHosts = hosts.length;
      
      // Auth tokens are the next N non-empty entries after host definitions
      let authTokenIndex = 0;
      let profileIndex = 0;
      let foundHostDefinitions = 0;
      
      for (const entry of hostEntries) {
        if (!entry.trim()) {
          // Empty entry, add undefined and continue
          if (authTokenIndex < numHosts) {
            authTokens.push(undefined);
            authTokenIndex++;
          } else if (profileIndex < numHosts) {
            profiles.push(undefined);
            profileIndex++;
          }
          continue;
        }
        
        // Check if this is a host definition
        const colonIndex = entry.indexOf(':');
        if (colonIndex !== -1) {
          const potentialUrl = entry.substring(colonIndex + 1);
          if (potentialUrl.startsWith('http://') || potentialUrl.startsWith('https://')) {
            foundHostDefinitions++;
            continue;
          }
        }
        
        // This is either an auth token or profile
        if (authTokenIndex < numHosts) {
          authTokens.push(entry.trim());
          authTokenIndex++;
        } else if (profileIndex < numHosts) {
          profiles.push(entry.trim());
          profileIndex++;
        }
      }
      
      // Fill in any remaining undefined values
      while (authTokens.length < numHosts) {
        authTokens.push(undefined);
      }
      while (profiles.length < numHosts) {
        profiles.push(undefined);
      }
      
      // Create host configurations
      for (let i = 0; i < hosts.length; i++) {
        const host = hosts[i];
        const auth_token = authTokens[i] || undefined;
        const profile = profiles[i] || undefined;
        
        this.hosts.set(host.id, { id: host.id, base_url: host.base_url, auth_token, profile });
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