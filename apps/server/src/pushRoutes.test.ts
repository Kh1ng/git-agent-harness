import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createServer } from './server.js';
import type { WebPushNotifications } from './webPush.js';
import type { ApnsNotifications } from './apns.js';

test('Web Push and APNs registration require authentication and mutation receipts', async () => {
  const previous = {
    token: process.env.COORDINATOR_TOKEN,
    insecure: process.env.GAH_ALLOW_INSECURE_HTTP,
    mutation: process.env.GAH_MUTATION_STORE_PATH
  };
  process.env.COORDINATOR_TOKEN = 'push-test-token';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  const mutationStore = mkdtempSync(join(tmpdir(), 'gah-push-routes-'));
  process.env.GAH_MUTATION_STORE_PATH = mutationStore;
  let registered = false;
  const push = {
    publicKey: () => 'public-key',
    list: () => ({ count: registered ? 1 : 0 }),
    register: () => { registered = true; return { id: 'a'.repeat(24), count: 1 }; },
    remove: () => { registered = false; return { removed: true, count: 0 }; }
  } as unknown as WebPushNotifications;
  const apns = {
    list: () => ({ count: registered ? 1 : 0 }),
    register: () => { registered = true; return { id: 'b'.repeat(24), count: 1 }; },
    remove: () => { registered = false; return { removed: true, count: 0 }; }
  } as unknown as ApnsNotifications;
  const server = http.createServer(createServer({ webPushNotifications: push, apnsNotifications: apns }));
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const base = `http://127.0.0.1:${address.port}`;
  const remote = { 'X-Forwarded-For': '198.51.100.8' };
  const auth = { ...remote, Authorization: 'Bearer push-test-token' };
  try {
    assert.equal((await fetch(`${base}/api/push/public-key`, { headers: remote })).status, 401);
    assert.deepEqual(await (await fetch(`${base}/api/push/public-key`, { headers: auth })).json(), { publicKey: 'public-key' });
    assert.equal((await fetch(`${base}/api/push/apns-devices`, { headers: remote })).status, 401);
    const created = await fetch(`${base}/api/push/subscriptions`, {
      method: 'POST',
      headers: { ...auth, 'Content-Type': 'application/json', 'Idempotency-Key': 'push-register-0001' },
      body: JSON.stringify({ subscription: { endpoint: 'https://push.example/one', keys: { p256dh: 'p', auth: 'a' } } })
    });
    assert.equal(created.status, 201);
    assert.equal((await created.json() as { id: string }).id, 'a'.repeat(24));
    const removed = await fetch(`${base}/api/push/subscriptions/${'a'.repeat(24)}`, {
      method: 'DELETE', headers: { ...auth, 'Idempotency-Key': 'push-remove-00001' }
    });
    assert.equal(removed.status, 200);
    const apnsCreated = await fetch(`${base}/api/push/apns-devices`, {
      method: 'POST',
      headers: { ...auth, 'Content-Type': 'application/json', 'Idempotency-Key': 'apns-register-0001' },
      body: JSON.stringify({ token: 'c'.repeat(64) })
    });
    assert.equal(apnsCreated.status, 201);
    assert.equal((await apnsCreated.json() as { id: string }).id, 'b'.repeat(24));
    assert.equal((await fetch(`${base}/api/push/apns-devices/${'b'.repeat(24)}`, {
      method: 'DELETE', headers: { ...auth, 'Idempotency-Key': 'apns-remove-00001' }
    })).status, 200);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(mutationStore, { recursive: true, force: true });
    for (const [key, value] of Object.entries({ COORDINATOR_TOKEN: previous.token, GAH_ALLOW_INSECURE_HTTP: previous.insecure, GAH_MUTATION_STORE_PATH: previous.mutation })) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  }
});
