import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { existsSync, writeFileSync, unlinkSync } from 'node:fs';
import { resolve } from 'node:path';
import crypto from 'node:crypto';

import { createServer } from './server.js';
import { RegistryService, containsSecretWords, isSchemaCompatible } from './registryService.js';
import { getCoordinatorIdentity, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import type { RegisteredNode, NodeSummary, NodeHealthCheckResult } from '@git-agent-harness/contracts';

// Helper to set up temporary registry file
function createTempRegistryFile(): string {
  const tmpPath = resolve(process.cwd(), `config-test-registry-${crypto.randomBytes(6).toString('hex')}.json`);
  writeFileSync(tmpPath, JSON.stringify({ nodes: [] }, null, 2));
  return tmpPath;
}

// Mock node server
class MockNodeServer {
  server: http.Server;
  port: number = 0;
  behavior: (req: http.IncomingMessage, res: http.ServerResponse) => void = () => {};

  constructor() {
    this.server = http.createServer((req, res) => {
      this.behavior(req, res);
    });
  }

  async start(): Promise<number> {
    await new Promise<void>((resolve) => {
      this.server.listen(0, '127.0.0.1', () => {
        const addr = this.server.address() as AddressInfo;
        this.port = addr.port;
        resolve();
      });
    });
    return this.port;
  }

  async stop(): Promise<void> {
    await new Promise<void>((resolve) => {
      this.server.close(() => resolve());
    });
  }
}

// Helper to run client requests
async function makeRequest(
  baseUrl: string,
  path: string,
  method: string = 'GET',
  body?: any,
  headers: Record<string, string> = {}
) {
  const url = `${baseUrl}${path}`;
  const response = await fetch(url, {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...headers
    },
    body: body ? JSON.stringify(body) : undefined
  });
  
  let json: any = null;
  try {
    json = await response.json();
  } catch {
    // ignore
  }

  return { status: response.status, body: json };
}

// ---------------------------------------------------------------------------
// Unit tests for helper functions
// ---------------------------------------------------------------------------

test('containsSecretWords detects secret strings', () => {
  assert.equal(containsSecretWords('Node-1-Key'), true);
  assert.equal(containsSecretWords('token-auth'), true);
  assert.equal(containsSecretWords('secretNode'), true);
  assert.equal(containsSecretWords('SafeDisplay'), false);
  assert.equal(containsSecretWords('Agent-Harness'), false);
});

test('isSchemaCompatible validates digests', () => {
  const validSha = crypto.createHash('sha256').update('test').digest('hex');
  assert.equal(isSchemaCompatible(validSha), true);
  assert.equal(isSchemaCompatible('gah-node-v1-digest'), true);
  assert.equal(isSchemaCompatible('gah-compat-v2'), true);
  assert.equal(isSchemaCompatible('invalid_digest'), false);
});

// ---------------------------------------------------------------------------
// Coordinator Identity tests
// ---------------------------------------------------------------------------

test('getCoordinatorIdentity returns stable identity', () => {
  const tempPath = resolve(process.cwd(), `config-test-identity-${crypto.randomBytes(6).toString('hex')}.json`);
  
  try {
    resetCachedCoordinatorIdentity();
    const id1 = getCoordinatorIdentity(tempPath);
    resetCachedCoordinatorIdentity();
    const id2 = getCoordinatorIdentity(tempPath);

    assert.equal(id1.node_id, id2.node_id);
    assert.equal(id1.display_name, 'GAH Coordinator');
    assert.equal(id1.version, '0.1.0');
    assert.ok(id1.schema_digest);
  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

// ---------------------------------------------------------------------------
// Registry Service Validation tests
// ---------------------------------------------------------------------------

test('RegistryService rejects duplicate IDs and malformed inputs', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);

  try {
    const validNode: RegisteredNode = {
      node_id: 'node-1',
      display_name: 'Safe Display Name',
      advertised_url: 'http://localhost:8080',
      version: '0.1.0',
      schema_digest: crypto.createHash('sha256').update('schema').digest('hex'),
      transport_mode: 'loopback',
      secret_ref: 'env:NODE_1_SECRET'
    };

    registry.registerNode(validNode);

    // Duplicate node_id
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        advertised_url: 'http://localhost:8081'
      });
    }, /Duplicate node ID/);

    // Collision
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-2'
      });
    }, /Endpoint collision/);

    // Secret looking display name
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-3',
        display_name: 'Secret-Key-Node',
        advertised_url: 'http://localhost:8082'
      });
    }, /contains secret-looking words/);

    // Secret looking label
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-3',
        advertised_url: 'http://localhost:8082',
        labels: ['auth-token']
      });
    }, /contains secret-looking words/);

    // Raw credential in secret_ref
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-3',
        advertised_url: 'http://localhost:8082',
        secret_ref: 'raw-unsecured-password'
      });
    }, /Secret reference must use references/);

    // Incompatible schema
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-3',
        advertised_url: 'http://localhost:8082',
        schema_digest: 'not-a-digest'
      });
    }, /Incompatible schema/);

  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

test('RegistryService validates non-loopback endpoints and TLS modes', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);

  try {
    const baseNode: Omit<RegisteredNode, 'advertised_url' | 'transport_mode'> = {
      node_id: 'node-remote',
      display_name: 'Remote Node',
      version: '0.1.0',
      schema_digest: crypto.createHash('sha256').update('schema').digest('hex'),
      secret_ref: 'env:NODE_SECRET'
    };

    // Non-loopback URL + loopback transport_mode -> Fail
    assert.throws(() => {
      registry.registerNode({
        ...baseNode,
        advertised_url: 'http://node.remote.com',
        transport_mode: 'loopback'
      });
    }, /cannot use loopback transport mode/);

    // Non-loopback URL + authenticated_remote + no TLS -> Fail
    assert.throws(() => {
      registry.registerNode({
        ...baseNode,
        advertised_url: 'http://node.remote.com',
        transport_mode: 'authenticated_remote'
      });
    }, /must use TLS/);

    // Non-loopback URL + authenticated_remote + TLS -> Success
    const resRemoteTls = registry.registerNode({
      ...baseNode,
      node_id: 'node-remote-tls',
      advertised_url: 'https://node.remote.com',
      transport_mode: 'authenticated_remote'
    });
    assert.equal(resRemoteTls.warnings.length, 0);

    // Non-loopback URL + trusted_lan + no TLS -> Warn, but Success
    const resLan = registry.registerNode({
      ...baseNode,
      node_id: 'node-lan',
      advertised_url: 'http://node.lan.com',
      transport_mode: 'trusted_lan'
    });
    assert.ok(resLan.warnings.length > 0);
    assert.ok(resLan.warnings[0].includes('Trusted-LAN endpoints on non-loopback addresses without TLS'));

  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

// ---------------------------------------------------------------------------
// Health check status mapping tests
// ---------------------------------------------------------------------------

test('checkNodeHealth distinguishes different failure kinds', async () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);
  const mockNode = new MockNodeServer();
  const mockPort = await mockNode.start();

  const nodeObj: RegisteredNode = {
    node_id: 'mock-node',
    display_name: 'Mock Node',
    advertised_url: `http://127.0.0.1:${mockPort}`,
    version: '0.1.0',
    schema_digest: crypto.createHash('sha256').update('schema-data').digest('hex'),
    transport_mode: 'authenticated_remote',
    secret_ref: 'env:MOCK_NODE_TOKEN'
  };

  registry.registerNode(nodeObj);
  process.env.MOCK_NODE_TOKEN = 'mock-bearer-token';

  try {
    // 1. DNS Failure: Point to a domain that doesn't exist
    const dnsNode: RegisteredNode = {
      ...nodeObj,
      node_id: 'dns-fail-node',
      advertised_url: 'https://does-not-exist-at-all-12345.xyz',
      secret_ref: 'env:MOCK_NODE_TOKEN'
    };
    registry.registerNode(dnsNode);
    const dnsRes = await registry.checkNodeHealth('dns-fail-node');
    assert.equal(dnsRes.status, 'unhealthy');
    assert.equal(dnsRes.error?.kind, 'DNS');

    // 2. Network connection failure: Point to port that is closed
    const netNode: RegisteredNode = {
      ...nodeObj,
      node_id: 'net-fail-node',
      advertised_url: 'http://127.0.0.1:48281'
    };
    registry.registerNode(netNode);
    const netRes = await registry.checkNodeHealth('net-fail-node');
    assert.equal(netRes.status, 'unhealthy');
    assert.equal(netRes.error?.kind, 'NETWORK');

    // 3. Auth failure: Server responds 401/403
    mockNode.behavior = (req, res) => {
      res.writeHead(401, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'Unauthorized' }));
    };
    const authRes = await registry.checkNodeHealth('mock-node');
    assert.equal(authRes.status, 'unhealthy');
    assert.equal(authRes.error?.kind, 'AUTH');

    // 4. Protocol failure: Server returns HTML/text or non-200
    mockNode.behavior = (req, res) => {
      res.writeHead(500, { 'Content-Type': 'text/plain' });
      res.end('Server internal error');
    };
    const protoRes1 = await registry.checkNodeHealth('mock-node');
    assert.equal(protoRes1.status, 'unhealthy');
    assert.equal(protoRes1.error?.kind, 'PROTOCOL');

    mockNode.behavior = (req, res) => {
      res.writeHead(200, { 'Content-Type': 'text/plain' });
      res.end('Plain text');
    };
    const protoRes2 = await registry.checkNodeHealth('mock-node');
    assert.equal(protoRes2.status, 'unhealthy');
    assert.equal(protoRes2.error?.kind, 'PROTOCOL');

    // 5. Version failure: version mismatch
    mockNode.behavior = (req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        status: 'healthy',
        version: '0.2.0', // different major/minor
        schema_digest: nodeObj.schema_digest
      }));
    };
    const verRes = await registry.checkNodeHealth('mock-node');
    assert.equal(verRes.status, 'unhealthy');
    assert.equal(verRes.error?.kind, 'VERSION');

    // 6. Schema failure: digest mismatch
    mockNode.behavior = (req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        status: 'healthy',
        version: '0.1.5',
        schema_digest: 'wrong-digest'
      }));
    };
    const schemaRes = await registry.checkNodeHealth('mock-node');
    assert.equal(schemaRes.status, 'unhealthy');
    assert.equal(schemaRes.error?.kind, 'SCHEMA');

    // 7. Success: status 200, correct version and digest
    mockNode.behavior = (req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        status: 'healthy',
        version: '0.1.5',
        schema_digest: nodeObj.schema_digest
      }));
    };
    const successRes = await registry.checkNodeHealth('mock-node');
    assert.equal(successRes.status, 'healthy');
    assert.equal(successRes.error, undefined);

  } finally {
    delete process.env.MOCK_NODE_TOKEN;
    await mockNode.stop();
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

// ---------------------------------------------------------------------------
// Server API Integration / Auth / TLS tests
// ---------------------------------------------------------------------------

test('Server endpoints enforce loopback check and authentication', async () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);
  
  process.env.COORDINATOR_TOKEN = 'expected-coordinator-token';

  const app = createServer({ registryService: registry });
  const server = http.createServer(app);

  await new Promise<void>((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve());
  });

  const { port } = server.address() as AddressInfo;
  const baseUrl = `http://127.0.0.1:${port}`;

  try {
    // 1. Local loopback request bypasses auth
    const localRes = await makeRequest(baseUrl, '/api/registry/nodes');
    assert.equal(localRes.status, 200);
    assert.ok(Array.isArray(localRes.body));

    // To simulate a remote non-loopback request, we can mock the request IP / headers or test middleware logic.
    // However, since express detects loopback by parsing req.ip (which fetch to 127.0.0.1 will produce as local),
    // let's test the middleware behavior by overriding clientIp detection.
    // In our authMiddleware, we check req.ip / req.socket.remoteAddress.
    // If we test with headers, can we override? No, express sets req.ip from socket.
    // Wait, let's spin up the server listening on a mock public-like interface if possible? Or we can configure
    // express 'trust proxy' and set 'X-Forwarded-For' to a non-loopback IP like '8.8.8.8'!
    // Let's check: in Express, if we enable `app.set('trust proxy', true)`, we can pass `X-Forwarded-For: 8.8.8.8`.
    // Let's modify the app in the test or mock the IP.
    // Let's check: does createServer enable proxy trust? It doesn't, but we can call it on the app instance!
    app.set('trust proxy', true);

    // 2. Non-loopback request: no TLS -> returns 403 Forbidden
    const headersNoTls = {
      'X-Forwarded-For': '8.8.8.8' // Simulates remote client
    };
    const remoteNoTlsRes = await makeRequest(baseUrl, '/api/registry/nodes', 'GET', undefined, headersNoTls);
    assert.equal(remoteNoTlsRes.status, 403);
    assert.equal(remoteNoTlsRes.body.error, 'Forbidden');

    // 3. Non-loopback request: TLS but no auth -> returns 401 Unauthorized
    const headersTlsNoAuth = {
      'X-Forwarded-For': '8.8.8.8',
      'X-Forwarded-Proto': 'https' // Simulates TLS behind reverse proxy
    };
    const remoteTlsNoAuthRes = await makeRequest(baseUrl, '/api/registry/nodes', 'GET', undefined, headersTlsNoAuth);
    assert.equal(remoteTlsNoAuthRes.status, 401);
    assert.equal(remoteTlsNoAuthRes.body.error, 'Unauthorized');

    // 4. Non-loopback request: TLS and wrong token -> returns 401 Unauthorized
    const headersWrongToken = {
      'X-Forwarded-For': '8.8.8.8',
      'X-Forwarded-Proto': 'https',
      'Authorization': 'Bearer wrong-token-value'
    };
    const remoteWrongTokenRes = await makeRequest(baseUrl, '/api/registry/nodes', 'GET', undefined, headersWrongToken);
    assert.equal(remoteWrongTokenRes.status, 401);

    // 5. Non-loopback request: TLS and correct token -> returns 200 Success
    const headersCorrect = {
      'X-Forwarded-For': '8.8.8.8',
      'X-Forwarded-Proto': 'https',
      'Authorization': 'Bearer expected-coordinator-token'
    };
    const remoteSuccessRes = await makeRequest(baseUrl, '/api/registry/nodes', 'GET', undefined, headersCorrect);
    assert.equal(remoteSuccessRes.status, 200);
    assert.ok(Array.isArray(remoteSuccessRes.body));

  } finally {
    delete process.env.COORDINATOR_TOKEN;
    await new Promise<void>((resolve) => server.close(() => resolve()));
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

// ---------------------------------------------------------------------------
// CRUD, Rotation, and Revocation API integration tests
// ---------------------------------------------------------------------------

test('Server endpoints handle Node CRUD, Secret Rotation and Revocation', async () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);
  
  const app = createServer({ registryService: registry });
  const server = http.createServer(app);

  await new Promise<void>((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve());
  });

  const { port } = server.address() as AddressInfo;
  const baseUrl = `http://127.0.0.1:${port}`;

  try {
    const nodeObj: RegisteredNode = {
      node_id: 'test-api-node',
      display_name: 'Test API Node',
      advertised_url: 'http://localhost:9000',
      version: '0.1.0',
      schema_digest: crypto.createHash('sha256').update('schema-data').digest('hex'),
      transport_mode: 'loopback',
      secret_ref: 'env:TEST_SECRET'
    };

    // 1. Register node (POST /api/registry/nodes)
    const registerRes = await makeRequest(baseUrl, '/api/registry/nodes', 'POST', nodeObj);
    assert.equal(registerRes.status, 201);
    assert.equal(registerRes.body.success, true);

    // 2. Verify registered node exists (GET /api/registry/nodes)
    const listRes = await makeRequest(baseUrl, '/api/registry/nodes');
    assert.equal(listRes.status, 200);
    assert.equal(listRes.body.length, 1);
    assert.equal(listRes.body[0].node_id, 'test-api-node');
    // Ensure secrets are NOT exposed
    assert.equal(listRes.body[0].secret_ref, undefined);

    // 3. Rotate Secret (POST /api/registry/nodes/:nodeId/rotate-secret)
    const rotateRes = await makeRequest(baseUrl, `/api/registry/nodes/${nodeObj.node_id}/rotate-secret`, 'POST', {
      secret_ref: 'env:ROTATED_SECRET'
    });
    assert.equal(rotateRes.status, 200);
    assert.equal(rotateRes.body.success, true);
    
    // Validate secret was updated
    const updatedNode = registry.getNode(nodeObj.node_id);
    assert.equal(updatedNode?.secret_ref, 'env:ROTATED_SECRET');

    // 4. Revoke Node (DELETE /api/registry/nodes/:nodeId)
    const revokeRes = await makeRequest(baseUrl, `/api/registry/nodes/${nodeObj.node_id}`, 'DELETE');
    assert.equal(revokeRes.status, 200);
    assert.equal(revokeRes.body.success, true);

    // Verify list is now empty
    const listResEmpty = await makeRequest(baseUrl, '/api/registry/nodes');
    assert.equal(listResEmpty.status, 200);
    assert.equal(listResEmpty.body.length, 0);

  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});
