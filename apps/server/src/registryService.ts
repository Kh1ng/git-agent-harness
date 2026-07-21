import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import crypto from 'node:crypto';
import type { RegisteredNode, NodeSummary, NodeHealthCheckResult } from '@git-agent-harness/contracts';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

export function isLoopback(urlStr: string): boolean {
  try {
    const url = new URL(urlStr);
    const host = url.hostname;
    return (
      host === 'localhost' ||
      host === '127.0.0.1' ||
      host === '::1' ||
      host.startsWith('127.') ||
      host === '[::1]'
    );
  } catch {
    return false;
  }
}

export function getEndpoint(urlStr: string): string {
  try {
    const url = new URL(urlStr);
    const port = url.port || (url.protocol === 'https:' || url.protocol === 'wss:' ? '443' : '80');
    return `${url.hostname}:${port}`;
  } catch {
    return urlStr;
  }
}

export function containsSecretWords(text: string): boolean {
  const secretPatterns = [/key/i, /secret/i, /password/i, /token/i, /cert/i, /credential/i, /auth/i, /private/i];
  return secretPatterns.some((pattern) => pattern.test(text));
}

export function isSchemaCompatible(schemaDigest: string): boolean {
  return schemaDigest === COORDINATOR_SCHEMA_DIGEST;
}

export function resolveSecret(secretRef: string): string {
  if (!secretRef) {
    throw new Error('Secret reference is empty');
  }
  if (secretRef.startsWith('env:')) {
    const envVar = secretRef.slice(4);
    const val = process.env[envVar];
    if (val === undefined) {
      throw new Error(`Environment variable ${envVar} is not set`);
    }
    return val;
  }
  if (secretRef.startsWith('file:')) {
    const filePath = secretRef.slice(5);
    try {
      return readFileSync(filePath, 'utf8').trim();
    } catch (e: any) {
      throw new Error(`Failed to read secret file ${filePath}: ${e.message}`);
    }
  }
  throw new Error(`Unsupported secret reference format (must start with 'env:' or 'file:')`);
}

export class RegistryService {
  private configPath: string;
  private nodes: Map<string, RegisteredNode> = new Map();

  constructor(configPath?: string) {
    this.configPath = configPath || process.env.GAH_REGISTRY_CONFIG_PATH || resolve(process.cwd(), 'config/registry-config.json');
    this.load();
  }

  private load() {
    if (existsSync(this.configPath)) {
      try {
        const data = JSON.parse(readFileSync(this.configPath, 'utf8'));
        if (Array.isArray(data.nodes)) {
          for (const node of data.nodes) {
            this.nodes.set(node.node_id, node);
          }
        }
      } catch (e) {
        console.error('Failed to load registry config:', e);
      }
    }
  }

  private save() {
    try {
      const dir = dirname(this.configPath);
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }
      const data = {
        nodes: Array.from(this.nodes.values())
      };
      writeFileSync(this.configPath, JSON.stringify(data, null, 2));
    } catch (e) {
      console.error('Failed to save registry config:', e);
      throw e;
    }
  }

  getNodes(): RegisteredNode[] {
    return Array.from(this.nodes.values());
  }

  getNode(nodeId: string): RegisteredNode | undefined {
    return this.nodes.get(nodeId);
  }

  getNodesSummary(): NodeSummary[] {
    return this.getNodes().map(({ secret_ref, ...summary }) => summary);
  }

  registerNode(node: RegisteredNode): { warnings: string[] } {
    const warnings: string[] = [];

    // 1. Basic validation
    if (!node.node_id || typeof node.node_id !== 'string') {
      throw new Error('Invalid or missing node_id');
    }
    if (!node.display_name || typeof node.display_name !== 'string') {
      throw new Error('Invalid or missing display_name');
    }
    if (!node.advertised_url || typeof node.advertised_url !== 'string') {
      throw new Error('Invalid or missing advertised_url');
    }
    if (!node.version || typeof node.version !== 'string') {
      throw new Error('Invalid or missing version');
    }
    if (!node.schema_digest || typeof node.schema_digest !== 'string') {
      throw new Error('Invalid or missing schema_digest');
    }

    // 2. Reject duplicate IDs
    if (this.nodes.has(node.node_id)) {
      throw new Error(`Duplicate node ID: ${node.node_id} is already registered`);
    }

    // 3. Reject endpoint collisions
    const newEndpoint = getEndpoint(node.advertised_url);
    for (const existingNode of this.nodes.values()) {
      if (getEndpoint(existingNode.advertised_url) === newEndpoint) {
        throw new Error(`Endpoint collision: ${node.advertised_url} collides with registered node ${existingNode.node_id}`);
      }
    }

    // 4. Reject incompatible schema
    if (!isSchemaCompatible(node.schema_digest)) {
      throw new Error(`Incompatible schema digest: ${node.schema_digest}`);
    }

    // 5. Reject secret-looking labels
    if (containsSecretWords(node.display_name)) {
      throw new Error(`Display name '${node.display_name}' contains secret-looking words`);
    }
    if (node.labels) {
      for (const label of node.labels) {
        if (containsSecretWords(label)) {
          throw new Error(`Label '${label}' contains secret-looking words`);
        }
      }
    }

    // 6. Registry config supports certificate/token secret references, not raw credentials
    if (!node.secret_ref || (!node.secret_ref.startsWith('env:') && !node.secret_ref.startsWith('file:'))) {
      throw new Error('Secret reference must use references (starting with env: or file:), not raw credentials');
    }

    // 7. Non-loopback endpoints require TLS plus authenticated node/client identity; localhost development remains explicit
    const loopback = isLoopback(node.advertised_url);
    if (!loopback) {
      if (node.transport_mode === 'loopback') {
        throw new Error('Non-loopback advertised URL cannot use loopback transport mode');
      }
      if (node.transport_mode === 'authenticated_remote') {
        const url = node.advertised_url.toLowerCase();
        if (!url.startsWith('https://') && !url.startsWith('wss://')) {
          throw new Error('Non-loopback authenticated remote endpoints must use TLS (https:// or wss://)');
        }
      } else if (node.transport_mode === 'trusted_lan') {
        throw new Error('Non-loopback advertised URL cannot use trusted_lan transport mode; use authenticated_remote with TLS');
      }
    } else {
      if (node.transport_mode === 'authenticated_remote') {
        const url = node.advertised_url.toLowerCase();
        if (!url.startsWith('https://') && !url.startsWith('wss://')) {
          warnings.push('Loopback authenticated remote endpoint does not terminate TLS locally');
        }
      }
    }

    this.nodes.set(node.node_id, node);
    this.save();
    return { warnings };
  }

  revokeNode(nodeId: string): boolean {
    const deleted = this.nodes.delete(nodeId);
    if (deleted) {
      this.save();
    }
    return deleted;
  }

  rotateSecret(nodeId: string, secretRef: string): void {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }
    if (!secretRef || (!secretRef.startsWith('env:') && !secretRef.startsWith('file:'))) {
      throw new Error('Secret reference must use references (starting with env: or file:), not raw credentials');
    }
    node.secret_ref = secretRef;
    this.save();
  }

  async checkNodeHealth(nodeId: string): Promise<NodeHealthCheckResult> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }

    const start = Date.now();
    const healthUrl = `${node.advertised_url}/health`;

    const headers: Record<string, string> = {
      'Accept': 'application/json',
      'User-Agent': 'GAH-Coordinator/0.1.0'
    };

    if (node.transport_mode === 'authenticated_remote') {
      let token = '';
      try {
        token = resolveSecret(node.secret_ref);
      } catch (e: any) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: start,
          error: {
            kind: 'AUTH',
            message: `Failed to resolve secret reference: ${e.message}`
          }
        };
      }
      headers['Authorization'] = `Bearer ${token}`;
    }

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 5000);

      const response = await fetch(healthUrl, {
        headers,
        signal: controller.signal
      }).finally(() => clearTimeout(timeoutId));

      if (response.status === 401 || response.status === 403) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'AUTH',
            message: `Node returned HTTP ${response.status} (Unauthorized)`
          }
        };
      }

      if (!response.ok) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'PROTOCOL',
            message: `Node returned HTTP status ${response.status}`
          }
        };
      }

      const contentType = response.headers.get('content-type') || '';
      if (!contentType.includes('application/json')) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'PROTOCOL',
            message: `Node returned non-JSON content-type: ${contentType}`
          }
        };
      }

      let data: any;
      try {
        data = await response.json();
      } catch (e: any) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'PROTOCOL',
            message: `Failed to parse JSON response: ${e.message}`
          }
        };
      }

      if (!data || typeof data !== 'object') {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'PROTOCOL',
            message: 'Node health response is not an object'
          }
        };
      }

      const nodeVersion = data.version;
      if (!nodeVersion || typeof nodeVersion !== 'string') {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'VERSION',
            message: 'Node health response missing version'
          }
        };
      }

      const expectedMajor = '0';
      const expectedMinor = '1';
      const parts = nodeVersion.split('.');
      if (parts[0] !== expectedMajor || parts[1] !== expectedMinor) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'VERSION',
            message: `Incompatible node version: ${nodeVersion}. Expected ${expectedMajor}.${expectedMinor}.x`
          }
        };
      }

      const nodeSchemaDigest = data.schema_digest || (data.identity && data.identity.schema_digest);
      if (!nodeSchemaDigest || typeof nodeSchemaDigest !== 'string') {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'SCHEMA',
            message: 'Node health response missing schema_digest'
          }
        };
      }

      if (nodeSchemaDigest !== node.schema_digest) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'SCHEMA',
            message: `Schema digest mismatch. Registered: ${node.schema_digest}, node reported: ${nodeSchemaDigest}`
          }
        };
      }

      return {
        node_id: node.node_id,
        status: 'healthy',
        timestamp: Date.now()
      };

    } catch (err: any) {
      let errorMsg = err.message || String(err);
      let errorCode = err.code || '';
      let causeCode = '';
      let causeMsg = '';

      if (err.cause) {
        causeCode = err.cause.code || '';
        causeMsg = err.cause.message || String(err.cause);
        errorMsg = `${errorMsg} (Cause: ${causeMsg})`;
      }

      if (
        errorCode === 'ENOTFOUND' ||
        errorCode === 'EAI_AGAIN' ||
        causeCode === 'ENOTFOUND' ||
        causeCode === 'EAI_AGAIN' ||
        errorMsg.includes('ENOTFOUND') ||
        errorMsg.includes('EAI_AGAIN')
      ) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'DNS',
            message: `DNS lookup failed: ${errorMsg}`
          }
        };
      }

      if (
        errorCode === 'ECONNREFUSED' ||
        errorCode === 'ETIMEDOUT' ||
        errorCode === 'ECONNRESET' ||
        errorCode === 'EHOSTUNREACH' ||
        errorCode === 'ENETUNREACH' ||
        causeCode === 'ECONNREFUSED' ||
        causeCode === 'ETIMEDOUT' ||
        causeCode === 'ECONNRESET' ||
        causeCode === 'EHOSTUNREACH' ||
        causeCode === 'ENETUNREACH' ||
        err.name === 'AbortError' ||
        (err.cause && err.cause.name === 'TimeoutError')
      ) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'NETWORK',
            message: `Network connection failed: ${errorMsg}`
          }
        };
      }

      if (
        (typeof errorCode === 'string' && (errorCode.includes('SSL') || errorCode.includes('CERT'))) ||
        (typeof causeCode === 'string' && (causeCode.includes('SSL') || causeCode.includes('CERT'))) ||
        errorMsg.toLowerCase().includes('ssl') ||
        errorMsg.toLowerCase().includes('certificate') ||
        errorMsg.toLowerCase().includes('tls')
      ) {
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          timestamp: Date.now(),
          error: {
            kind: 'TLS',
            message: `TLS validation failed: ${errorMsg}`
          }
        };
      }

      return {
        node_id: node.node_id,
        status: 'unhealthy',
        timestamp: Date.now(),
        error: {
          kind: 'PROTOCOL',
          message: `HTTP request failed: ${errorMsg} (code: ${errorCode}, causeCode: ${causeCode})`
        }
      };
    }
  }
}
