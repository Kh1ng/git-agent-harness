import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { existsSync, writeFileSync, unlinkSync } from 'node:fs';
import { resolve } from 'node:path';
import crypto from 'node:crypto';
import os from 'node:os';

import { createServer } from './server.js';
import { RegistryService, containsSecretWords, isSchemaCompatible } from './registryService.js';
import { COORDINATOR_SCHEMA_DIGEST, getCoordinatorIdentity, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { authMiddleware, isLocalAddress } from './authMiddleware.js';
import { ClaimsService } from './claimsService.js';
import type { RegisteredNode, NodeSummary, NodeHealthCheckResult } from '@git-agent-harness/contracts';


// Helper to set up temporary registry file
function createTempRegistryFile(): string {
  const tmpPath = resolve(process.cwd(), `config-test-registry-${crypto.randomBytes(6).toString('hex')}.json`);
  writeFileSync(tmpPath, JSON.stringify({ nodes: [] }, null, 2));
  return tmpPath;
}

function statusPayload(overrides: Record<string, any> = {}) {
  return {
    schema_version: 1,
    review_contract_version: 1,
    generated_at: '2026-07-21T12:00:00Z',
    profile: {
      profile: 'gah',
      display_name: 'GAH Node',
      repo_id: 'owner/repo',
      provider: 'github',
      local_path: '/tmp/repo',
      default_target_branch: 'main',
      merge_policy: 'auto',
      max_fix_attempts_per_mr: 2,
      max_implementation_failures_per_ticket: 8,
      max_open_managed_mrs: 1,
      issue_intake_policy: {
        mode: 'legacy',
        canonical_autonomous_label: 'exec:autonomous',
        trusted_human_authors: [],
        trusted_bot_authors: [],
        github_issue_author_allowlist: []
      }
    },
    observations: {
      sync: { status: 'ok' },
      availability: { status: 'ok' },
      ledger: { status: 'ok' }
    },
    merge_requests: [],
    availability: [],
    recent_ledger: null,
    constraints: [],
    blockers: [],
    blocked_work_items: [],
    issue_intake_rejections: [],
    dependency_blockers: [],
    errors: [],
    available_tickets: [],
    active_claims: [],
    pm_parent_states: [],
    pm_decomposition_attempt_counts: {},
    pm_max_attempts: 2,
    fix_attempt_counts: {},
    merge_attempt_counts: {},
    review_held_work_ids: [],
    publishing_allow_pr: true,
    generated_artifact_deny_patterns: [],
    max_parallel_workers: 1,
    open_managed_mr_count: 0,
    inflight_implementation_count: 0,
    implementation_intake_paused: false,
    backend_configured: {},
    backend_instances: [],
    resource_pressure: {
      cpu_percent: 5,
      rss_bytes: 123456,
      disk_percent: 7
    },
    event_cursor: '2026-07-21T12:00:00Z',
    ...overrides
  };
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
  assert.equal(isSchemaCompatible(COORDINATOR_SCHEMA_DIGEST), true);
  assert.equal(isSchemaCompatible(crypto.createHash('sha256').update('test').digest('hex')), false);
  assert.equal(isSchemaCompatible('gah-node-v1-digest'), false);
  assert.equal(isSchemaCompatible('invalid_digest'), false);
});

// ---------------------------------------------------------------------------
// Coordinator Identity tests
// ---------------------------------------------------------------------------

test('getCoordinatorIdentity returns stable identity', () => {
  const tempPath = resolve(process.cwd(), `config-test-identity-${crypto.randomBytes(6).toString('hex')}.json`);
  
  try {
    resetCachedCoordinatorIdentity();
    const id1 = getCoordinatorIdentity(tempPath, 9123);
    resetCachedCoordinatorIdentity();
    const id2 = getCoordinatorIdentity(tempPath, 9123);

    assert.equal(id1.node_id, id2.node_id);
    assert.equal(id1.display_name, 'GAH Coordinator');
    assert.equal(id1.advertised_url, 'http://localhost:9123');
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

test('RegistryService updates duplicate IDs and rejects malformed inputs', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);

  try {
    const validNode: RegisteredNode = {
      node_id: 'node-1',
      display_name: 'Safe Display Name',
      advertised_url: 'http://localhost:8080',
      version: '0.1.0',
      schema_digest: COORDINATOR_SCHEMA_DIGEST,
      transport_mode: 'loopback',
      secret_ref: 'env:NODE_1_SECRET'
    };

    registry.registerNode(validNode);

    const update = registry.registerNode({
      ...validNode,
      advertised_url: 'http://localhost:8081',
      profiles: ['gah']
    });
    assert.equal(update.created, false);
    assert.deepEqual(registry.getNode(validNode.node_id)?.profiles, ['gah']);

    // Collision
    assert.throws(() => {
      registry.registerNode({
        ...validNode,
        node_id: 'node-2',
        advertised_url: 'http://localhost:8081'
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
        schema_digest: crypto.createHash('sha256').update('not-the-current-schema').digest('hex')
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
      schema_digest: COORDINATOR_SCHEMA_DIGEST,
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

    // Non-loopback URL + trusted_lan -> Fail closed by default (issue #944)
    const prevAllow = process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
    delete process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
    try {
      assert.throws(() => {
        registry.registerNode({
          ...baseNode,
          node_id: 'node-lan',
          advertised_url: 'http://node.lan.com',
          transport_mode: 'trusted_lan'
        });
      }, /GAH_REGISTRY_ALLOW_INSECURE_LAN=1/);
    } finally {
      if (prevAllow === undefined) {
        delete process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
      } else {
        process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN = prevAllow;
      }
    }

    // Non-loopback URL + trusted_lan + explicit opt-in -> Success (with warning)
    process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN = '1';
    try {
      const resLan = registry.registerNode({
        ...baseNode,
        node_id: 'node-lan-opted',
        advertised_url: 'http://node.lan.com',
        transport_mode: 'trusted_lan'
      });
      assert.equal(resLan.warnings.length, 1);
    } finally {
      delete process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
    }

  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

test('RegistryService rejects a node advertising the central node\'s own endpoint (self-poll guard, issue #944)', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath, 'http://localhost:3773');

  try {
    const baseNode: Omit<RegisteredNode, 'advertised_url' | 'transport_mode'> = {
      node_id: 'self-poll-node',
      display_name: 'Bad Node',
      version: '0.1.0',
      schema_digest: COORDINATOR_SCHEMA_DIGEST,
      secret_ref: 'env:NODE_SECRET'
    };

    // Advertises the central's own loopback endpoint -> rejected
    assert.throws(() => {
      registry.registerNode({
        ...baseNode,
        advertised_url: 'http://127.0.0.1:3773',
        transport_mode: 'loopback'
      });
    }, /central node's own endpoint/);

    // Different host:port that is NOT the central -> accepted
    const ok = registry.registerNode({
      ...baseNode,
      node_id: 'legit-node',
      advertised_url: 'http://127.0.0.1:9999',
      transport_mode: 'loopback'
    });
    assert.equal(ok.warnings.length, 0);
  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

test('RegistryService rejects its listener when the central advertises through a reverse proxy', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath, 'https://central.example.com', 3773);

  try {
    assert.throws(() => {
      registry.registerNode({
        node_id: 'self-poll-alias',
        display_name: 'Bad Node',
        advertised_url: 'http://127.0.0.1:3773',
        version: '0.1.0',
        schema_digest: COORDINATOR_SCHEMA_DIGEST,
        transport_mode: 'loopback',
        secret_ref: 'env:NODE_SECRET'
      });
    }, /central node's own endpoint/);

    for (const [nodeId, advertisedUrl] of [
      ['self-poll-public-host', 'http://central.example.com:3773'],
      ['self-poll-ipv4-wildcard', 'http://0.0.0.0:3773'],
      ['self-poll-ipv6-wildcard', 'http://[::]:3773']
    ]) {
      assert.throws(() => registry.registerNode({
        node_id: nodeId,
        display_name: 'Bad Node',
        advertised_url: advertisedUrl,
        version: '0.1.0',
        schema_digest: COORDINATOR_SCHEMA_DIGEST,
        transport_mode: 'trusted_lan',
        secret_ref: 'env:NODE_SECRET'
      }), /central node's own endpoint/);
    }
  } finally {
    if (existsSync(tempPath)) unlinkSync(tempPath);
  }
});

test('RegistryService rejects a local interface behind a reverse proxy', () => {
  const address = Object.values(os.networkInterfaces()).flat().find((entry) => entry && !entry.internal)?.address;
  assert.ok(address, 'test host must expose a non-loopback interface');
  const host = address.includes(':') ? `[${address}]` : address;
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath, 'https://central.example.com', 3773);

  try {
    assert.throws(() => registry.registerNode({
      node_id: 'self-poll-interface',
      display_name: 'Bad Node',
      advertised_url: `http://${host}:3773`,
      version: '0.1.0',
      schema_digest: COORDINATOR_SCHEMA_DIGEST,
      transport_mode: 'trusted_lan',
      secret_ref: 'env:NODE_SECRET'
    }), /central node's own endpoint/);
  } finally {
    if (existsSync(tempPath)) unlinkSync(tempPath);
  }
});

test('RegistryService rejects an unrecognized transport_mode instead of silently skipping TLS enforcement', () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);

  try {
    assert.throws(() => {
      registry.registerNode({
        node_id: 'node-bogus-mode',
        display_name: 'Bogus Mode Node',
        advertised_url: 'http://node.remote.com',
        version: '0.1.0',
        schema_digest: COORDINATOR_SCHEMA_DIGEST,
        secret_ref: 'env:NODE_SECRET',
        // Not a member of the transport_mode union -- must fail closed, not
        // fall through the loopback/authenticated_remote/trusted_lan chain
        // unenforced.
        transport_mode: 'carrier-pigeon' as RegisteredNode['transport_mode']
      });
    }, /Invalid transport_mode/);
  } finally {
    if (existsSync(tempPath)) {
      unlinkSync(tempPath);
    }
  }
});

test('resolveSecret rejects file: references outside the configured secrets root', async () => {
  const { mkdirSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const testRoot = resolve(tmpdir(), `gah-node-secrets-test-${crypto.randomBytes(6).toString('hex')}`);
  const previousRoot = process.env.GAH_NODE_SECRETS_ROOT;
  process.env.GAH_NODE_SECRETS_ROOT = testRoot;

  try {
    mkdirSync(testRoot, { recursive: true });
    const { resolveSecret } = await import('./registryService.js');

    // Outside the root entirely, and a `../` traversal attempt out of the root.
    assert.throws(() => resolveSecret('file:/etc/passwd'), /must be inside/);
    assert.throws(() => resolveSecret(`file:${testRoot}/../escape.txt`), /must be inside/);

    const allowedPath = resolve(testRoot, 'allowed-secret.txt');
    writeFileSync(allowedPath, 'super-secret-value\n');
    assert.equal(resolveSecret(`file:${allowedPath}`), 'super-secret-value');
  } finally {
    if (previousRoot === undefined) {
      delete process.env.GAH_NODE_SECRETS_ROOT;
    } else {
      process.env.GAH_NODE_SECRETS_ROOT = previousRoot;
    }
    rmSync(testRoot, { recursive: true, force: true });
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
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
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

test('getNodeObservations preserves stale, auth-failed, incompatible, and deduplicated node work identities', async () => {
  const tempPath = createTempRegistryFile();
  const registry = new RegistryService(tempPath);
  const healthyNode = new MockNodeServer();
  const staleNode = new MockNodeServer();
  const incompatibleNode = new MockNodeServer();
  const healthyGeneratedAt = new Date(Date.now() - 5_000).toISOString();
  const staleGeneratedAt = new Date(Date.now() - (31 * 60 * 1000)).toISOString();

  const healthyPort = await healthyNode.start();
  const stalePort = await staleNode.start();
  const incompatiblePort = await incompatibleNode.start();

  const healthyNodeObj: RegisteredNode = {
    node_id: 'healthy-node',
    display_name: 'Healthy Node',
    advertised_url: `http://127.0.0.1:${healthyPort}`,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:HEALTHY_NODE_TOKEN'
  };
  const staleNodeObj: RegisteredNode = {
    node_id: 'stale-node',
    display_name: 'Stale Node',
    advertised_url: `http://127.0.0.1:${stalePort}`,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:STALE_NODE_TOKEN'
  };
  const incompatibleNodeObj: RegisteredNode = {
    node_id: 'incompatible-node',
    display_name: 'Incompatible Node',
    advertised_url: `http://127.0.0.1:${incompatiblePort}`,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:INCOMPATIBLE_NODE_TOKEN'
  };
  const authFailedNodeObj: RegisteredNode = {
    node_id: 'auth-failed-node',
    display_name: 'Dormant Node',
    advertised_url: 'http://127.0.0.1:9',
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'authenticated_remote',
    secret_ref: 'env:DOES_NOT_EXIST'
  };

  registry.registerNode(healthyNodeObj);
  registry.registerNode(staleNodeObj);
  registry.registerNode(incompatibleNodeObj);
  registry.registerNode(authFailedNodeObj);

  healthyNode.behavior = (_req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(statusPayload({
      generated_at: healthyGeneratedAt,
      active_claims: [
        {
          work_id: 'TICKET-1',
          node_id: 'healthy-node',
          scope: 'gah@test',
          hostname: 'host-a',
          claimed_at: '2026-07-21T11:59:50Z',
          age_seconds: 8
        },
        {
          work_id: 'TICKET-1',
          node_id: 'healthy-node',
          scope: 'gah@test',
          hostname: 'host-a',
          claimed_at: '2026-07-21T11:59:50Z',
          age_seconds: 8
        }
      ]
    })));
  };

  staleNode.behavior = (_req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(statusPayload({
      generated_at: staleGeneratedAt
    })));
  };

  incompatibleNode.behavior = (_req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(statusPayload({
      version: '0.2.0',
      schema_digest: 'wrong-digest'
    })));
  };

  process.env.HEALTHY_NODE_TOKEN = 'healthy-token';
  process.env.STALE_NODE_TOKEN = 'stale-token';
  process.env.INCOMPATIBLE_NODE_TOKEN = 'incompatible-token';

  try {
    const observations = await registry.getNodeObservations('gah');
    const byNode = new Map(observations.map((observation) => [observation.node_id, observation]));

    assert.equal(byNode.get('healthy-node')?.state, 'healthy');
    assert.equal(byNode.get('healthy-node')?.active_work.length, 1);
    assert.equal(byNode.get('healthy-node')?.active_work[0]?.node_qualified_work_id, 'healthy-node:TICKET-1');

    assert.equal(byNode.get('stale-node')?.state, 'stale');
    assert.equal(byNode.get('stale-node')?.last_seen_at, staleGeneratedAt);

    assert.equal(byNode.get('incompatible-node')?.state, 'incompatible');
    assert.equal(byNode.get('auth-failed-node')?.state, 'auth_failed');
    assert.equal(byNode.get('auth-failed-node')?.last_seen_at, null);

    const listRes = registry.getNodesSummary();
    const authSummary = listRes.find((node) => node.node_id === 'auth-failed-node');
    assert.equal(authSummary?.last_observed_state, 'auth_failed');
  } finally {
    delete process.env.HEALTHY_NODE_TOKEN;
    delete process.env.STALE_NODE_TOKEN;
    delete process.env.INCOMPATIBLE_NODE_TOKEN;
    await healthyNode.stop();
    await staleNode.stop();
    await incompatibleNode.stop();
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

  // Bind 0.0.0.0 so genuinely non-loopback connections are possible. The
  // authMiddleware trusts only the TCP socket address (authMiddleware.ts), so a
  // spoofed X-Forwarded-For from a loopback socket can no longer simulate a
  // remote client the way it did before the socket-only change.
  await new Promise<void>((resolve) => {
    server.listen(0, '0.0.0.0', () => resolve());
  });

  const { port } = server.address() as AddressInfo;
  const loopbackUrl = `http://127.0.0.1:${port}`;

  const interfaces = os.networkInterfaces();
  const nonLoopbackIp = Object.values(interfaces)
    .flatMap((list) => list ?? [])
    .find((addr) => addr?.family === 'IPv4' && !isLocalAddress(addr.address))
    ?.address;

  try {
    // 1. Local loopback request bypasses auth
    const localRes = await makeRequest(loopbackUrl, '/api/registry/nodes');
    assert.equal(localRes.status, 200);
    assert.ok(Array.isArray(localRes.body));

    // Only trust X-Forwarded-* from a loopback hop, never from any direct peer
    // (see server.ts) -- otherwise a remote attacker could forge
    // X-Forwarded-Proto: https and defeat the TLS requirement below.
    assert.equal(app.get('trust proxy'), 'loopback');

    // 2. A loopback socket stays trusted even when it forges proxy headers --
    // this is the Caddy reverse-proxy path, where the real client IP arrives
    // in X-Forwarded-For.
    const forgedHeaders = {
      'X-Forwarded-For': '8.8.8.8',
      'X-Forwarded-Proto': 'https'
    };
    const proxyPathRes = await makeRequest(loopbackUrl, '/api/registry/nodes', 'GET', undefined, forgedHeaders);
    assert.equal(proxyPathRes.status, 200);

    // 3. Genuinely non-loopback connection: no TLS -> 403 Forbidden. Forged
    // X-Forwarded-Proto is ignored because trust proxy is 'loopback', so the
    // TLS requirement can never be satisfied by header spoofing from a direct
    // peer.
    if (!nonLoopbackIp) {
      // No routable interface in this environment (e.g. some CI containers);
      // the loopback half of the contract above is still fully exercised.
      return;
    }
    const remoteUrl = `http://${nonLoopbackIp}:${port}`;

    const remoteNoTlsRes = await makeRequest(remoteUrl, '/api/registry/nodes', 'GET', undefined, forgedHeaders);
    assert.equal(remoteNoTlsRes.status, 403);
    assert.equal(remoteNoTlsRes.body.error, 'Forbidden');

    // 4. Same direct peer, even with a forged Bearer token -> still 403 (not
    // 401): the token path is only reachable over real TLS termination.
    const remoteWithTokenRes = await makeRequest(remoteUrl, '/api/registry/nodes', 'GET', undefined, {
      ...forgedHeaders,
      'Authorization': 'Bearer expected-coordinator-token'
    });
    assert.equal(remoteWithTokenRes.status, 403);

    // 5. The fleet snapshot endpoint enforces the same non-loopback gate.
    const remoteStatusRes = await makeRequest(remoteUrl, '/api/status', 'GET', undefined, forgedHeaders);
    assert.equal(remoteStatusRes.status, 403);

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
  const claimsPath = resolve(process.cwd(), `config-test-claims-${crypto.randomBytes(6).toString('hex')}.json`);
  const registry = new RegistryService(tempPath);
  
  const app = createServer({ registryService: registry, claimsService: new ClaimsService(claimsPath) });
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
      schema_digest: COORDINATOR_SCHEMA_DIGEST,
      transport_mode: 'loopback',
      secret_ref: 'env:TEST_SECRET'
    };

    // 1. Register node (POST /api/registry/nodes)
    const registerRes = await makeRequest(baseUrl, '/api/registry/nodes', 'POST', nodeObj);
    assert.equal(registerRes.status, 201);
    assert.equal(registerRes.body.success, true);

    const duplicateRes = await makeRequest(baseUrl, '/api/registry/nodes', 'POST', {
      ...nodeObj,
      profiles: ['gah']
    });
    assert.equal(duplicateRes.status, 200);
    assert.deepEqual(registry.getNode(nodeObj.node_id)?.profiles, ['gah']);

    const claimRes = await makeRequest(baseUrl, '/api/claims/acquire', 'POST', {
      node_id: nodeObj.node_id,
      profile: 'gah',
      work_id: 'upserted-profile-check'
    });
    assert.equal(claimRes.status, 200);

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
    if (existsSync(claimsPath)) {
      unlinkSync(claimsPath);
    }
  }
});

// ---------------------------------------------------------------------------
// authMiddleware Unit Tests
// ---------------------------------------------------------------------------

test('authMiddleware rejects non-loopback requests with spoofed X-Forwarded-Proto header if trust proxy is disabled', () => {
  const req = {
    ip: '8.8.8.8',
    headers: {
      'x-forwarded-proto': 'https',
      'authorization': 'Bearer expected-coordinator-token'
    },
    secure: false, // Express sets this to false because trust proxy is disabled
    socket: {
      remoteAddress: '8.8.8.8'
    }
  } as any;

  let statusCalledWith: number | null = null;
  let jsonCalledWith: any = null;
  let nextCalled = false;

  const res = {
    status: (code: number) => {
      statusCalledWith = code;
      return {
        json: (data: any) => {
          jsonCalledWith = data;
        }
      };
    }
  } as any;

  const next = () => {
    nextCalled = true;
  };

  process.env.COORDINATOR_TOKEN = 'expected-coordinator-token';

  authMiddleware(req, res, next);

  assert.equal(nextCalled, false);
  assert.equal(statusCalledWith, 403);
  assert.equal(jsonCalledWith?.error, 'Forbidden');
  assert.equal(
    jsonCalledWith?.message,
    'Non-loopback endpoints require TLS unless GAH_REGISTRY_ALLOW_INSECURE_LAN=1'
  );
  
  delete process.env.COORDINATOR_TOKEN;
});

test('authMiddleware accepts non-loopback requests when req.secure is true and token is correct', () => {
  const req = {
    ip: '8.8.8.8',
    headers: {
      'authorization': 'Bearer expected-coordinator-token'
    },
    secure: true,
    socket: {
      remoteAddress: '8.8.8.8'
    }
  } as any;

  let statusCalledWith: number | null = null;
  let jsonCalledWith: any = null;
  let nextCalled = false;

  const res = {
    status: (code: number) => {
      statusCalledWith = code;
      return {
        json: (data: any) => {
          jsonCalledWith = data;
        }
      };
    }
  } as any;

  const next = () => {
    nextCalled = true;
  };

  process.env.COORDINATOR_TOKEN = 'expected-coordinator-token';

  authMiddleware(req, res, next);

  assert.equal(nextCalled, true);
  assert.equal(statusCalledWith, null);
  
  delete process.env.COORDINATOR_TOKEN;
});

test('authMiddleware accepts opted-in non-loopback HTTP with a valid token', () => {
  const previousAllow = process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
  const previousToken = process.env.COORDINATOR_TOKEN;
  process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN = '1';
  process.env.COORDINATOR_TOKEN = 'expected-token';
  const req = {
    socket: { remoteAddress: '100.64.0.2' },
    secure: false,
    headers: { authorization: 'Bearer expected-token' }
  } as any;
  let status: number | undefined;
  const res = {
    status(code: number) { status = code; return this; },
    json() { return this; }
  } as any;
  let called = false;

  try {
    authMiddleware(req, res, () => { called = true; });
    assert.equal(called, true);
    assert.equal(status, undefined);
  } finally {
    if (previousAllow === undefined) delete process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN;
    else process.env.GAH_REGISTRY_ALLOW_INSECURE_LAN = previousAllow;
    if (previousToken === undefined) delete process.env.COORDINATOR_TOKEN;
    else process.env.COORDINATOR_TOKEN = previousToken;
  }
});

test('authMiddleware does not treat spoofed loopback headers as local on a remote socket', () => {
  const req = {
    ip: '127.0.0.1',
    headers: {
      'x-forwarded-for': '127.0.0.1',
      'x-forwarded-proto': 'https'
    },
    secure: true,
    socket: {
      remoteAddress: '203.0.113.10'
    }
  } as any;

  let statusCalledWith: number | null = null;
  let jsonCalledWith: any = null;
  let nextCalled = false;

  const res = {
    status: (code: number) => {
      statusCalledWith = code;
      return {
        json: (data: any) => {
          jsonCalledWith = data;
        }
      };
    }
  } as any;

  const next = () => {
    nextCalled = true;
  };

  process.env.COORDINATOR_TOKEN = 'expected-coordinator-token';

  authMiddleware(req, res, next);

  assert.equal(nextCalled, false);
  assert.equal(statusCalledWith, 401);
  assert.equal(jsonCalledWith?.error, 'Unauthorized');
  assert.equal(jsonCalledWith?.message, 'Authentication token required for non-loopback access');

  delete process.env.COORDINATOR_TOKEN;
});

test('authMiddleware timing-safe comparison rejects invalid tokens', () => {
  const req = {
    ip: '8.8.8.8',
    headers: {
      'authorization': 'Bearer wrong-token-value'
    },
    secure: true,
    socket: {
      remoteAddress: '8.8.8.8'
    }
  } as any;

  let statusCalledWith: number | null = null;
  let jsonCalledWith: any = null;
  let nextCalled = false;

  const res = {
    status: (code: number) => {
      statusCalledWith = code;
      return {
        json: (data: any) => {
          jsonCalledWith = data;
        }
      };
    }
  } as any;

  const next = () => {
    nextCalled = true;
  };

  process.env.COORDINATOR_TOKEN = 'expected-coordinator-token';

  authMiddleware(req, res, next);

  assert.equal(nextCalled, false);
  assert.equal(statusCalledWith, 401);
  assert.equal(jsonCalledWith?.error, 'Unauthorized');
  
  delete process.env.COORDINATOR_TOKEN;
});
