import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from './server.js';
import { RegistryService } from './registryService.js';
import { ClaimsService } from './claimsService.js';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

const snapshot = { schema_version: 1, generated_at: new Date().toISOString(), overall_status: 'fail',
  checks: [{ profile: 'other', name: 'provider auth', status: 'fail', detail: 'Run gh auth login on the worker.' }] };

async function listen(server: http.Server): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

async function close(server: http.Server) {
  server.closeAllConnections();
  await new Promise<void>((resolve) => server.close(() => resolve()));
}

test('node doctor authenticates each boundary, rejects undeclared profiles, and returns failed checks as data', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-node-doctor-'));
  const previous = { central: process.env.COORDINATOR_TOKEN, worker: process.env.GAH_NODE_TOKEN, insecure: process.env.GAH_ALLOW_INSECURE_HTTP };
  process.env.COORDINATOR_TOKEN = 'central-test-token';
  process.env.GAH_NODE_TOKEN = 'worker-secret-canary';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  let mode = 'valid';
  let requests = 0;
  const worker = http.createServer((req, res) => {
    requests++;
    assert.equal(req.url, '/api/doctor?profile=other');
    assert.equal(req.headers.authorization, 'Bearer worker-secret-canary');
    res.setHeader('Content-Type', 'application/json');
    if (mode === 'unauthorized') { res.writeHead(401); res.end('worker-secret-canary'); }
    else if (mode === 'slow-body') { res.writeHead(200); res.flushHeaders(); }
    else res.end(JSON.stringify(mode === 'invalid-profile' ? { ...snapshot, checks: [{ ...snapshot.checks[0], profile: 'different' }] } : snapshot));
  });
  const workerUrl = await listen(worker);
  const registry = new RegistryService(join(dir, 'registry.json'));
  registry.registerNode({ node_id: 'worker', display_name: 'Worker', advertised_url: workerUrl, version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'authenticated_remote', secret_ref: 'env:GAH_NODE_TOKEN', profiles: ['other'] });
  const app = createServer({ registryService: registry, claimsService: new ClaimsService(join(dir, 'claims.json')) });
  const central = http.createServer((req, res) => {
    // Exercise the real non-loopback auth path without using the machine's network.
    Object.defineProperty(req.socket, 'remoteAddress', { value: '198.51.100.10', configurable: true });
    app(req, res);
  });
  const base = await listen(central);
  const read = (path = '/api/registry/nodes/worker/doctor?profile=other', authenticated = true) => fetch(`${base}${path}`, {
    headers: authenticated ? { Authorization: 'Bearer central-test-token' } : {}
  });
  try {
    assert.equal((await read(undefined, false)).status, 401);
    assert.equal((await read('/api/registry/nodes/missing/doctor?profile=other')).status, 404);
    assert.equal((await read('/api/registry/nodes/worker/doctor?profile=undeclared')).status, 400);
    assert.equal((await read('/api/registry/nodes/worker/doctor')).status, 400);
    assert.equal(requests, 0);
    const result = await read();
    assert.equal(result.status, 200);
    assert.equal(result.headers.get('cache-control'), 'no-store');
    assert.deepEqual(await result.json(), snapshot);
    mode = 'invalid-profile';
    assert.match(await (await read()).text(), /"message":"PROTOCOL:/);
    mode = 'unauthorized';
    const unauthorized = await (await read()).text();
    assert.match(unauthorized, /AUTH:/);
    assert.equal(unauthorized.includes('worker-secret-canary'), false);
    mode = 'slow-body';
    const timeout = AbortSignal.timeout.bind(AbortSignal);
    const mocked = t.mock.method(AbortSignal, 'timeout', () => timeout(25));
    try {
      const timedOut = await read();
      assert.equal(timedOut.status, 504, 'the deadline also covers a stalled JSON body');
      assert.match(await timedOut.text(), /"message":"TIMEOUT:/);
    } finally { mocked.mock.restore(); }
    await close(worker);
    const offline = await read();
    assert.equal(offline.status, 502);
    assert.match(await offline.text(), /"message":"NETWORK:/);
  } finally {
    await close(central); await close(worker);
    for (const [name, value] of Object.entries({ COORDINATOR_TOKEN: previous.central, GAH_NODE_TOKEN: previous.worker, GAH_ALLOW_INSECURE_HTTP: previous.insecure })) {
      if (value === undefined) delete process.env[name]; else process.env[name] = value;
    }
    rmSync(dir, { recursive: true });
  }
});

test('a removed worker cannot publish an in-flight readiness result', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-node-doctor-race-'));
  let deliver = () => {};
  let arrived = () => {};
  const started = new Promise<void>((resolve) => { arrived = resolve; });
  const worker = http.createServer((_req, res) => {
    deliver = () => { res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(snapshot)); };
    arrived();
  });
  const workerUrl = await listen(worker);
  const registry = new RegistryService(join(dir, 'registry.json'));
  registry.registerNode({ node_id: 'worker', display_name: 'Worker', advertised_url: workerUrl, version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:UNUSED', profiles: ['other'] });
  try {
    const pending = registry.checkNodeDoctor('worker', 'other');
    await started;
    registry.revokeNode('worker');
    deliver();
    await assert.rejects(pending, /registration changed/);
  } finally { await close(worker); rmSync(dir, { recursive: true }); }
});
