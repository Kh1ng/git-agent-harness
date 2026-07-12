import { readFile } from 'fs/promises';
import path from 'path';
import { fileURLToPath } from 'url';
import fetch from 'node-fetch';

export interface HostConfig {
  id: string;
  name: string;
  base_url: string;
  auth_token?: string;
  profile: string;
}

export interface HostRegistryConfig {
  hosts: HostConfig[];
}

export interface HostStatus {
  host_id: string;
  reachable: boolean;
  latency_ms?: number;
  status?: string;
  error?: string;
}

export class HostRegistry {
  private hosts: HostConfig[];
  private localHostId: string;
  private localHostName: string;

  constructor(config?: HostRegistryConfig) {
    this.hosts = config?.hosts || [];
    this.localHostId = process.env.HOST_ID || require('os').hostname();
    this.localHostName = process.env.HOST_NAME || this.localHostId;
  }

  static async loadFromFile(configPath?: string): Promise<HostRegistry> {
    try {
      const resolvedPath = configPath || process.env.GAH_HOSTS_CONFIG || 
        path.join(path.dirname(fileURLToPath(import.meta.url)), '../../hosts.json');
      
      const configContent = await readFile(resolvedPath, 'utf-8');
      const config: HostRegistryConfig = JSON.parse(configContent);
      return new HostRegistry(config);
    } catch (error: unknown) {
      if (error instanceof Error && (error as NodeJS.ErrnoException).code === 'ENOENT') {
        // Config file doesn't exist, return empty registry
        return new HostRegistry();
      }
      throw new Error(`Failed to load host registry: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  getLocalHostId(): string {
    return this.localHostId;
  }

  getLocalHostName(): string {
    return this.localHostName;
  }

  getAllHosts(): HostConfig[] {
    return [...this.hosts];
  }

  async checkHostHealth(hostId: string): Promise<HostStatus> {
    const host = this.hosts.find(h => h.id === hostId);
    
    if (!host) {
      return {
        host_id: hostId,
        reachable: false,
        error: 'Host not found in registry'
      };
    }

    // If this is the local host, check local health
    if (hostId === this.localHostId) {
      try {
        const response = await fetch('http://localhost:3773/health');
        if (response.ok) {
          const data = await response.json();
          return {
            host_id: hostId,
            reachable: true,
            status: data.status
          };
        }
        return {
          host_id: hostId,
          reachable: false,
          error: `Local health check failed: ${response.statusText}`
        };
      } catch (error) {
        return {
          host_id: hostId,
          reachable: false,
          error: error instanceof Error ? error.message : 'Unknown error'
        };
      }
    }

    // For remote hosts, probe their health endpoint
    try {
      const startTime = Date.now();
      const healthUrl = new URL('/health', host.base_url);
      const response = await fetch(healthUrl.toString(), {
        headers: host.auth_token ? { 'Authorization': `Bearer ${host.auth_token}` } : {}
      });
      const latency = Date.now() - startTime;
      
      if (response.ok) {
        const data = await response.json();
        return {
          host_id: hostId,
          reachable: true,
          latency_ms: latency,
          status: data.status
        };
      }
      
      return {
        host_id: hostId,
        reachable: false,
        latency_ms: latency,
        error: `Health check failed: ${response.status} ${response.statusText}`
      };
    } catch (error) {
      return {
        host_id: hostId,
        reachable: false,
        error: error instanceof Error ? error.message : 'Unknown error'
      };
    }
  }

  async checkAllHostsHealth(): Promise<HostStatus[]> {
    const results: HostStatus[] = [];
    
    // Always include local host first
    results.push(await this.checkHostHealth(this.localHostId));
    
    // Check all configured hosts
    for (const host of this.hosts) {
      if (host.id !== this.localHostId) {
        results.push(await this.checkHostHealth(host.id));
      }
    }
    
    return results;
  }

  getHostById(hostId: string): HostConfig | undefined {
    return this.hosts.find(h => h.id === hostId);
  }
}