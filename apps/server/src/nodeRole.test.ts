import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer as createHttpServer, type Server } from 'node:http';
import express from 'express';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { validateNodeRole, workerMemoryEnvironment, workerRouteGuard } from './nodeRole.js';
import { workerMemoryRouter } from './workerMemory.js';
import { RegistryService } from './registryService.js';
import { authMiddleware } from './authMiddleware.js';
import { initializeSkillBank, createServer } from './server.js';
import type { DoctorSnapshot } from '@git-agent-harness/contracts';

async function listen(server: Server): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  return `http://127.0.0.1:${address.port}`;
}
const close = (server: Server) => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
const worker = { role: 'worker', central_url: 'https://central.test' } as const;

test('worker role reports central identity and permits execution without hosting central stores', async () => {
  assert.throws(() => validateNodeRole({ role: 'worker', central_url: null }), /requires/);
  assert.throws(() => validateNodeRole({ role: 'worker', central_url: 'https://user:secret@central.test' }), /credentials/);
  assert.throws(() => validateNodeRole({ role: 'central', central_url: 'https://user:secret@central.test' }), /credentials/);
  assert.throws(() => initializeSkillBank(worker), /must not initialize/);
  const localOnlyRegistry = new RegistryService(null);
  assert.deepEqual(localOnlyRegistry.getNodesSummary(), []);
  assert.throws(() => workerMemoryEnvironment(worker, undefined), /COORDINATOR_TOKEN/);
  assert.deepEqual(workerMemoryEnvironment(worker, 'node-token'), { TDAI_GATEWAY_URL: 'https://central.test/api/worker-memory', TDAI_GATEWAY_API_KEY: 'node-token' });
  const app = createServer({ node: worker, runDoctor: async () => ({ schema_version: 1, generated_at: '2026-09-07T00:00:00Z', overall_status: 'ok', checks: [] } satisfies DoctorSnapshot) });
  const server = createHttpServer(app);
  const url = await listen(server);
  try {
    const health = await (await fetch(`${url}/health`)).json() as { node: unknown };
    assert.deepEqual(health.node, worker);
    for (const path of ['/api/settings/nodes/command', '/api/skills', '/api/claims/acquire', '/api/registry/nodes', '/api/manager-chat/sessions', '/api/worker-memory/recall', '/api/pm/plans']) {
      const response = await fetch(url + path);
      assert.equal(response.status, 409, path);
      assert.equal((await response.json() as { central_url: string }).central_url, worker.central_url);
    }
    assert.equal((await fetch(`${url}/api/doctor?profile=test`)).status, 200);
    // Invalid dispatch reaches execution validation; it never starts a backend.
    assert.equal((await fetch(`${url}/api/dispatch`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 400);
    const probe = express();
    probe.use(workerRouteGuard(worker));
    probe.get('/api/status', (_req, res) => res.sendStatus(200));
    const statusServer = createHttpServer(probe);
    const statusUrl = await listen(statusServer);
    try { assert.equal((await fetch(`${statusUrl}/api/status`)).status, 200); } finally { await close(statusServer); }
  } finally { await close(server); }
});

test('central memory relay preserves gateway contracts without forwarding worker credentials or arbitrary destinations', async () => {
  const previous = { url: process.env.TDAI_GATEWAY_URL, key: process.env.TDAI_GATEWAY_API_KEY, settings: process.env.GAH_GATEWAY_SETTINGS_PATH };
  const directory = mkdtempSync(join(tmpdir(), 'gah-relay-'));
  const received: { path?: string; authorization?: string; body?: unknown }[] = [];
  const gateway = express();
  gateway.use(express.json());
  gateway.post('*', (req, res) => {
    received.push({ path: req.path, authorization: req.headers.authorization, body: req.body });
    res.json(req.path === '/recall' ? { code: 0, context: 'shared memory', memory_count: 1 } : { l0_recorded: 1 });
  });
  const gatewayServer = createHttpServer(gateway);
  process.env.TDAI_GATEWAY_URL = await listen(gatewayServer);
  process.env.TDAI_GATEWAY_API_KEY = 'central-gateway-key';
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(directory, 'settings.json');
  const app = express();
  app.use(express.json());
  app.use('/api/worker-memory', authMiddleware, workerMemoryRouter());
  const server = createHttpServer(app);
  const url = await listen(server);
  try {
    for (const [operation, body] of [
      ['recall', { session_key: 'gah:worker:owner/repo:#1', query: 'previous work' }],
      ['capture', { session_key: 'gah:worker:owner/repo:#1', user_content: 'task', assistant_content: 'result' }],
      ['session/end', { session_key: 'gah:worker:owner/repo:#1' }],
    ] as const) {
      const response = await fetch(`${url}/api/worker-memory/${operation}`, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer worker-token' }, body: JSON.stringify({ ...body, profile: 'worker-project', url: 'https://attacker.test', token: 'worker-token' }) });
      assert.equal(response.status, 200);
      assert.deepEqual(received.at(-1), { path: `/${operation}`, authorization: 'Bearer central-gateway-key', body });
      assert.ok(!JSON.stringify(await response.json()).includes('central-gateway-key'));
    }
    const bad = await fetch(`${url}/api/worker-memory/recall`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ session_key: 'other', query: 'x' }) });
    assert.equal(bad.status, 400);
    assert.equal((await fetch(`${url}/api/worker-memory/arbitrary`, { method: 'POST' })).status, 404);
    assert.equal(received.length, 3);
    for (const settings of [{ enabled: false }, { disabledProfiles: ['worker-project'] }]) {
      writeFileSync(process.env.GAH_GATEWAY_SETTINGS_PATH!, JSON.stringify(settings));
      const skipped = await fetch(`${url}/api/worker-memory/capture`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ profile: 'worker-project', session_key: 'gah:worker:owner/repo:#1', user_content: 'task', assistant_content: 'result' }) });
      assert.equal(skipped.status, 200);
      assert.equal((await skipped.json() as { l0_recorded: number }).l0_recorded, 0);
      assert.equal(received.length, 3, 'Disabled memory must not contact the gateway');
    }
  } finally {
    await close(server);
    await close(gatewayServer);
    rmSync(directory, { recursive: true });
    for (const [key, value] of Object.entries({ TDAI_GATEWAY_URL: previous.url, TDAI_GATEWAY_API_KEY: previous.key, GAH_GATEWAY_SETTINGS_PATH: previous.settings })) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  }
});
