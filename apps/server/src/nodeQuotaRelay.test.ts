import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import type { AccountUsageObservation } from '@git-agent-harness/contracts';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { RegistryService } from './registryService.js';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';
import { createServer } from './server.js';

function quota() {
  const value = JSON.parse(readFileSync(new URL('../tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8'));
  value.profile.profile = 'gah';
  value.candidates[0].backend = 'claude';
  value.candidates[0].backend_instance = 'claude';
  value.candidates[0].provider = 'anthropic';
  return value;
}

async function listen(server: http.Server): Promise<string> {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

test('relay preserves scoped dashboard consumption and rejects invalid or credential-bearing metadata', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-quota-dashboard-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const usage: AccountUsageObservation = {
    account_id: 'customer-test', workspace_id: null, period_start: '2026-10-01T00:00:00.066000+00:00', period_end: '2026-10-03T00:00:00+00:00', currency: 'USD',
    requests: 42, input_tokens: 300, cached_input_tokens: 1000, output_tokens: 50, cost: 12.34, cost_source: 'dashboard_prices',
    models: [{ model: 'Vibe display alias', usage_type: 'vibe', requests: 42, input_tokens: 300, cached_input_tokens: 1000, output_tokens: 50, cost: 12.34 }]
  };
  let current: unknown = usage;
  const worker = http.createServer((_req, res) => {
    const snapshot = quota();
    snapshot.quota_checks[0].quota_observations = [{ backend: 'mistral-dashboard', usage_source: 'mistral_dashboard_session', account_usage: current }];
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(snapshot));
  });
  t.after(() => { worker.closeAllConnections(); worker.close(); });
  const workerUrl = await listen(worker);
  async function relay() {
    const registry = new RegistryService(join(directory, `registry-${Math.random()}.json`));
    registry.registerNode({ node_id: 'mac', display_name: 'Mac', advertised_url: workerUrl, version: '0.1.2', schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:UNUSED', profiles: ['gah'] });
    return (await registry.getNodeQuotas('gah', '7d')).nodes[0];
  }
  const valid = await relay();
  assert.equal(valid.state, 'available');
  assert.deepEqual(valid.quota?.quota_checks[0].quota_observations?.[0].account_usage, usage);
  assert.equal(valid.quota?.usage.entries, 42, 'provider consumption never replaces task accounting');
  for (const invalid of [
    { ...usage, requests: -1 }, { ...usage, input_tokens: 0.5 }, { ...usage, cost: -1 },
    { ...usage, cost: '12.34' }, { ...usage, currency: 'EUR' }, { ...usage, workspace_id: '' },
    { ...usage, period_start: 'bad' }, { ...usage, period_end: '2026-09-01T00:00:00Z' },
    { ...usage, period_start: '42' }, { ...usage, period_start: '2026-10-01' }, { ...usage, period_end: usage.period_start },
    { ...usage, account_id: 'x'.repeat(257) }, { ...usage, workspace_id: 'x'.repeat(257) },
    { ...usage, account_id: '😀'.repeat(65) }, { ...usage, workspace_id: undefined }, { ...usage, account_id: 'customer\u0085' },
    { ...usage, cost_source: undefined }, { ...usage, cost: undefined, cost_source: undefined },
    { ...usage, models: Array(129).fill(usage.models[0]) },
    { ...usage, models: [{ ...usage.models[0], model: 'x'.repeat(513) }] },
    { ...usage, cookie: 'private-dashboard-session' },
    { ...usage, models: [{ ...usage.models[0], usage_type: 'api_tokens' }] },
    { ...usage, models: [{ ...usage.models[0], requests: Number.MAX_SAFE_INTEGER + 1 }] },
    { ...usage, models: [{ ...usage.models[0], authorization: 'private-dashboard-session' }] }
  ]) {
    current = invalid;
    const rejected = await relay();
    assert.equal(rejected.state, 'unavailable');
    assert.equal(rejected.quota, null);
    assert.ok(!JSON.stringify(rejected).includes('private-dashboard-session'));
  }
});

test('registered-node quota relay authenticates, scopes, caches, and keeps worker accounting separate', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-quota-relay-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  process.env.QUOTA_RELAY_TEST_CREDENTIAL = 'worker-credential';
  t.after(() => { delete process.env.QUOTA_RELAY_TEST_CREDENTIAL; });
  let requests = 0;
  const worker = http.createServer((req, res) => {
    requests++;
    assert.equal(req.headers.authorization, 'Bearer worker-credential');
    assert.equal(req.url, '/api/quota?profile=gah&since=7d');
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(quota()));
  });
  t.after(() => { worker.closeAllConnections(); worker.close(); });
  const workerUrl = await listen(worker);
  const registry = new RegistryService(join(directory, 'registry.json'));
  const registration = {
    node_id: 'mac-worker', display_name: 'Mac', advertised_url: workerUrl,
    version: '0.1.2', schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'authenticated_remote' as const, secret_ref: 'env:QUOTA_RELAY_TEST_CREDENTIAL', profiles: ['gah'],
  };
  registry.registerNode(registration);
  const [first, second] = await Promise.all([registry.getNodeQuotas('gah', '7d'), registry.getNodeQuotas('gah', '7d')]);
  assert.deepEqual(first, second);
  assert.equal(requests, 1);
  assert.equal(first.nodes[0].nodeId, 'mac-worker');
  assert.equal(first.nodes[0].state, 'available');
  assert.equal(first.nodes[0].quota?.candidates[0].provider, 'anthropic');
  assert.equal(first.nodes[0].quota?.usage.entries, 42);
  assert.equal(registry.getCachedObservations().length, 0, 'quota reads must not publish dispatch health');
  const unsupported = await registry.getNodeQuotas('sportsball', '7d');
  assert.equal(unsupported.nodes[0].state, 'unsupported_profile');
  assert.equal(requests, 1);
  registry.revokeNode('mac-worker');
  assert.deepEqual((await registry.getNodeQuotas('gah', '7d')).nodes, []);

  registry.registerNode(registration);
  const app = createServer({ registryService: registry });
  const central = http.createServer(app);
  t.after(() => { central.closeAllConnections(); central.close(); registry.stopLivenessScheduler(); });
  const centralUrl = await listen(central);
  const response = await fetch(`${centralUrl}/api/registry/quota?profile=gah&since=7d`);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  const payload: unknown = await response.json();
  assert.ok(payload && typeof payload === 'object' && 'nodes' in payload && Array.isArray(payload.nodes));
  assert.equal(payload.nodes[0].nodeId, 'mac-worker');
  assert.equal((await fetch(`${centralUrl}/api/registry/quota?profile=gah&since=bad`)).status, 400);
  assert.equal((await fetch(`${centralUrl}/api/registry/quota?profile=gah`, { headers: { 'x-forwarded-proto': 'https' } })).status, 401);
});

test('a worker that stalls after headers cannot hold the quota relay beyond its deadline', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-quota-deadline-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const worker = http.createServer((_req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.write('{');
  });
  t.after(() => { worker.closeAllConnections(); worker.close(); });
  const registry = new RegistryService(join(directory, 'registry.json'));
  registry.registerNode({ node_id: 'slow', display_name: 'Slow worker', advertised_url: await listen(worker),
    version: '0.1.2', schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:UNUSED', profiles: ['gah'] });
  const started = Date.now();
  const result = await registry.getNodeQuotas('gah', '7d');
  assert.equal(result.nodes[0].state, 'unavailable');
  assert.ok(Date.now()-started < 8_000);
});

test('relay rejects mismatched schema, credentials, redirects, and excessive response bodies without echoing upstream secrets', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-quota-protocol-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const target = http.createServer((_req, res) => { assert.fail('redirect must not be followed'); res.end(); });
  t.after(() => { target.closeAllConnections(); target.close(); });
  const targetUrl = await listen(target);
  let behavior: (res: http.ServerResponse) => void = res => res.end();
  const worker = http.createServer((_req, res) => behavior(res));
  t.after(() => { worker.closeAllConnections(); worker.close(); });
  const workerUrl = await listen(worker);
  const invalid = quota(); invalid.profile.profile = 'different-profile';
  const invalidPercent = quota(); invalidPercent.candidates[0].quota_observations[0].quota_remaining_percent = 142;
  const invalidCheck = quota(); invalidCheck.quota_checks[0].quota_observations = [{ backend: 'opencode', quota_remaining_percent: 142 }];
  for (const write of [
    (res: http.ServerResponse) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(invalid)); },
    (res: http.ServerResponse) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(invalidPercent)); },
    (res: http.ServerResponse) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(invalidCheck)); },
    (res: http.ServerResponse) => { res.writeHead(401); res.end('upstream-private-token'); },
    (res: http.ServerResponse) => { res.writeHead(302, { location: targetUrl }); res.end(); },
    (res: http.ServerResponse) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end('x'.repeat(1024*1024+1)); },
  ]) {
    behavior = write;
    const registry = new RegistryService(join(directory, `registry-${Math.random()}.json`));
    registry.registerNode({ node_id: 'worker', display_name: 'Worker', advertised_url: workerUrl, version: '0.1.2',
      schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:UNUSED', profiles: ['gah'] });
    const result = await registry.getNodeQuotas('gah', '7d');
    assert.equal(result.nodes[0].state, 'unavailable');
    assert.equal(result.nodes[0].quota, null);
    assert.ok(!JSON.stringify(result).includes('upstream-private-token'));
  }
});
