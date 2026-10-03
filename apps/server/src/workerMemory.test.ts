import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer, type Server } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import { workerMemoryRouter } from './workerMemory.js';

async function listen(server: Server): Promise<string> {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  return `http://127.0.0.1:${address.port}`;
}

test('memory management enforces owner access and configured project membership', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-memory-management-'));
  const saved = Object.fromEntries(['TDAI_GATEWAY_URL', 'TDAI_GATEWAY_API_KEY', 'GAH_GATEWAY_SETTINGS_PATH', 'GAH_BINARY', 'GAH_FIXTURE_FAIL', 'GAH_FIXTURE_PROFILE_LIST'].map(key => [key, process.env[key]]));
  const received: Array<{ path: string; body: unknown }> = [];
  const gateway = express();
  gateway.use(express.json());
  gateway.post('*', (req, res) => {
    received.push({ path: req.path, body: req.body });
    res.json({ code: 0 });
  });
  const gatewayServer = createServer(gateway);
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const app = express();
  app.locals.deviceAccess = access;
  app.use(rateLimit({ windowMs: 60_000, limit: 60 }), express.json(), authMiddleware);
  app.use('/api/worker-memory', workerMemoryRouter());
  const server = createServer(app);
  t.after(async () => {
    await Promise.all([server, gatewayServer].map(server => new Promise<void>(resolve => server.close(() => resolve()))));
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(directory, { recursive: true, force: true });
  });
  process.env.TDAI_GATEWAY_URL = await listen(gatewayServer);
  process.env.TDAI_GATEWAY_API_KEY = 'test-gateway-key';
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(directory, 'settings.json');
  process.env.GAH_BINARY = fileURLToPath(new URL('../tests/fixtures/gah/gah', import.meta.url));
  delete process.env.GAH_FIXTURE_FAIL;
  delete process.env.GAH_FIXTURE_PROFILE_LIST;
  const origin = await listen(server);
  const offer = access.create({ id: 'test-node', name: 'Fixture', origin });
  const paired = access.redeem(offer.code, 'test-node', origin, 'Phone');
  const phoneHeaders = { Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}` };
  const body = { profile: 'fixture', session_key: 'gah:manager:fixture', id: 'memory-1' };
  const post = (operation: string, body: object, headers = {}) => fetch(`${origin}/api/worker-memory/${operation}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body),
  });

  for (const operation of ['memories/delete', 'memories/migrate-english']) {
    await t.test(`paired devices cannot invoke ${operation}`, async () => {
      const count = received.length;
      assert.equal((await post(operation, body, phoneHeaders)).status, 403);
      assert.equal(received.length, count, 'Rejected requests must not reach the gateway');
    });
  }
  await t.test('unknown profiles cannot bless arbitrary project session keys', async () => {
    const count = received.length;
    assert.equal((await post('memories/list', { profile: 'unknown-project', session_key: 'gah:manager:unknown-project' })).status, 400);
    assert.equal(received.length, count);
  });
  await t.test('profile lookup failure fails closed', async () => {
    const count = received.length;
    process.env.GAH_FIXTURE_FAIL = 'profile-list';
    try {
      assert.equal((await post('memories/delete', body)).status, 503);
      assert.equal(received.length, count);
    } finally { delete process.env.GAH_FIXTURE_FAIL; }
  });
  await t.test('configured owner management preserves the gateway contract', async () => {
    for (const operation of ['memories/delete', 'memories/migrate-english']) {
      assert.equal((await post(operation, body)).status, 200);
      assert.deepEqual(received.at(-1), { path: `/${operation}`, body: { session_key: body.session_key, ...(operation === 'memories/delete' ? { id: body.id } : {}) } });
    }
  });
  await t.test('paired capture and recall preserve ordinary worker session scopes', async () => {
    for (const [operation, fields] of [
      ['capture', { user_content: 'task', assistant_content: 'result' }],
      ['recall', { query: 'context' }],
    ] as const) {
      const session_key = 'gah:worker:owner/repo:#1';
      assert.equal((await post(operation, { profile: 'worker-project', session_key, ...fields }, phoneHeaders)).status, 200);
      assert.deepEqual(received.at(-1), { path: `/${operation}`, body: { session_key, ...fields } });
    }
  });
});
