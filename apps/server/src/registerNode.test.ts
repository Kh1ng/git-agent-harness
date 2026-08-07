// Issue #881: repeatable node-registration flow. Uses real fake HTTP
// servers (not mocked fetch) for both "self" (GET /health) and "central"
// (POST /api/registry/nodes), matching this repo's existing test
// convention (server.test.ts, memoryGatewayClient.test.ts).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';

import { registerNode } from './registerNode.js';

function listen(server: http.Server): Promise<string> {
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve(`http://127.0.0.1:${port}`);
    });
  });
}

function fakeSelfServer(identity: Record<string, unknown>): http.Server {
  return http.createServer((req, res) => {
    if (req.url === '/health') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(identity));
      return;
    }
    res.writeHead(404);
    res.end();
  });
}

/** Records every registration payload it receives; `rejectWith` lets a test
 * simulate the central registry's own validation (e.g. registerNode.ts's
 * duplicate-ID / schema-mismatch checks) without duplicating that logic. */
function fakeCentralServer(opts: {
  requireAuth?: string;
  rejectWith?: { status: number; message: string };
  received: Array<{ body: unknown; authorization: string | undefined }>;
}): http.Server {
  return http.createServer((req, res) => {
    let raw = '';
    req.on('data', (chunk) => (raw += chunk));
    req.on('end', () => {
      if (req.url !== '/api/registry/nodes' || req.method !== 'POST') {
        res.writeHead(404);
        res.end();
        return;
      }
      const authorization = req.headers.authorization;
      opts.received.push({ body: JSON.parse(raw), authorization });

      if (opts.requireAuth && authorization !== `Bearer ${opts.requireAuth}`) {
        res.writeHead(401, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: 'Unauthorized', message: 'bad token' }));
        return;
      }
      if (opts.rejectWith) {
        res.writeHead(opts.rejectWith.status, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: 'Bad Request', message: opts.rejectWith.message }));
        return;
      }
      res.writeHead(201, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ success: true, message: 'Node registered successfully', warnings: [] }));
    });
  });
}

const SAMPLE_IDENTITY = {
  node_id: 'node-abc',
  display_name: 'Test Node',
  advertised_url: 'http://127.0.0.1:3773',
  version: '0.1.0',
  schema_digest: 'deadbeef'
};

test('registers using this node\'s own /health identity, end-to-end against fake self+central servers', async () => {
  const self = fakeSelfServer(SAMPLE_IDENTITY);
  const received: Array<{ body: unknown; authorization: string | undefined }> = [];
  const central = fakeCentralServer({ received });
  const [selfUrl, centralUrl] = await Promise.all([listen(self), listen(central)]);

  try {
    const result = await registerNode({
      centralUrl,
      selfUrl,
      transportMode: 'loopback',
      secretRef: 'env:NODE_TOKEN'
    });

    assert.equal(result.node_id, 'node-abc');
    assert.equal(result.display_name, 'Test Node');
    assert.deepEqual(result.warnings, []);
    assert.equal(received.length, 1);
    assert.deepEqual(received[0].body, {
      node_id: 'node-abc',
      display_name: 'Test Node',
      advertised_url: 'http://127.0.0.1:3773',
      version: '0.1.0',
      schema_digest: 'deadbeef',
      transport_mode: 'loopback',
      secret_ref: 'env:NODE_TOKEN'
    });
  } finally {
    self.close();
    central.close();
  }
});

test('forwards COORDINATOR_TOKEN as Bearer auth to the central registry', async () => {
  const self = fakeSelfServer(SAMPLE_IDENTITY);
  const received: Array<{ body: unknown; authorization: string | undefined }> = [];
  const central = fakeCentralServer({ requireAuth: 'secret-token', received });
  const [selfUrl, centralUrl] = await Promise.all([listen(self), listen(central)]);

  try {
    await registerNode({
      centralUrl,
      selfUrl,
      transportMode: 'authenticated_remote',
      secretRef: 'env:NODE_TOKEN',
      token: 'secret-token'
    });
    assert.equal(received[0].authorization, 'Bearer secret-token');
  } finally {
    self.close();
    central.close();
  }
});

test('a rejected registration (e.g. wrong token, duplicate node) surfaces the central registry\'s error message', async () => {
  const self = fakeSelfServer(SAMPLE_IDENTITY);
  const received: Array<{ body: unknown; authorization: string | undefined }> = [];
  const central = fakeCentralServer({ requireAuth: 'right-token', received });
  const [selfUrl, centralUrl] = await Promise.all([listen(self), listen(central)]);

  try {
    await assert.rejects(
      registerNode({
        centralUrl,
        selfUrl,
        transportMode: 'authenticated_remote',
        secretRef: 'env:NODE_TOKEN',
        token: 'wrong-token'
      }),
      /bad token/
    );
  } finally {
    self.close();
    central.close();
  }
});

test('a duplicate/incompatible node is surfaced, not swallowed', async () => {
  const self = fakeSelfServer(SAMPLE_IDENTITY);
  const received: Array<{ body: unknown; authorization: string | undefined }> = [];
  const central = fakeCentralServer({
    received,
    rejectWith: { status: 400, message: 'Duplicate node ID: node-abc is already registered' }
  });
  const [selfUrl, centralUrl] = await Promise.all([listen(self), listen(central)]);

  try {
    await assert.rejects(
      registerNode({ centralUrl, selfUrl, transportMode: 'loopback', secretRef: 'env:NODE_TOKEN' }),
      /Duplicate node ID/
    );
  } finally {
    self.close();
    central.close();
  }
});

test('a self node that fails its own /health surfaces a clear error instead of registering garbage', async () => {
  const self = http.createServer((_req, res) => {
    res.writeHead(500);
    res.end();
  });
  const selfUrl = await listen(self);

  try {
    await assert.rejects(
      registerNode({
        centralUrl: 'http://127.0.0.1:1',
        selfUrl,
        transportMode: 'loopback',
        secretRef: 'env:NODE_TOKEN'
      }),
      /Failed to read own identity/
    );
  } finally {
    self.close();
  }
});
