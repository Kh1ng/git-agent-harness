import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ActivityEvent } from '@git-agent-harness/contracts';
import { ActivityFeed } from './activityFeed.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { createServer } from './server.js';

test('posted worker activity is authenticated, audited, bounded, and deduplicated', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-activity-route-'));
  const saved = { ...process.env };
  process.env.COORDINATOR_TOKEN = 'activity-route-token';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'identity.json');
  process.env.GAH_MUTATION_STORE_PATH = join(root, 'mutations');
  resetCachedCoordinatorIdentity();
  const delivered: string[] = [];
  const feed = new ActivityFeed(null, (event) => { delivered.push(event.id); });
  const server = http.createServer(createServer({ activityFeed: feed }));
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/api/activity`;
  const event: ActivityEvent = {
    id: 'worker:dispatch:run-42',
    occurredAt: '2026-09-26T12:00:00Z',
    profile: 'demo',
    kind: 'dispatch_failed',
    severity: 'error',
    title: 'Work failed',
    message: 'The worker exhausted its routes.',
    workId: '#42'
  };
  const post = (body: object, token = true, key = 'worker-activity-run-42') => fetch(url, {
    method: 'POST',
    headers: {
      Host: 'central.test',
      'X-Forwarded-For': '203.0.113.10',
      'Content-Type': 'application/json',
      'Idempotency-Key': key,
      ...(token ? { Authorization: 'Bearer activity-route-token' } : {})
    },
    body: JSON.stringify(body)
  });

  try {
    assert.equal((await post(event, false)).status, 401);
    assert.equal((await post({ ...event, message: 'x'.repeat(1_001) }, true, 'worker-activity-invalid-42')).status, 400);
    assert.equal((await post(event)).status, 201);
    assert.equal((await post(event)).status, 409);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(delivered, [event.id]);
    assert.match(readFileSync(join(root, 'mutations/audit.jsonl'), 'utf8'), /"operation":"activity.record"/);

    // #1273: the Notifications view lists it unread, and opening it marks it read.
    const remote = { Host: 'central.test', 'X-Forwarded-For': '203.0.113.10', Authorization: 'Bearer activity-route-token', 'Content-Type': 'application/json' };
    const list = async () => (await fetch(`${url}/notifications`, { headers: remote })).json() as Promise<{ events: ActivityEvent[]; unread: number }>;
    assert.deepEqual((await list()).events.map((item) => [item.id, item.readAt ?? null]), [[event.id, null]]);
    assert.equal((await fetch(`${url}/notifications`, { headers: { Host: 'central.test', 'X-Forwarded-For': '203.0.113.10' } })).status, 401);
    const read = (body: object) => fetch(`${url}/read`, { method: 'POST', headers: remote, body: JSON.stringify(body) });
    assert.equal((await read({ ids: [] })).status, 400);
    assert.deepEqual(await (await read({ ids: [event.id] })).json(), { changed: 1, unread: 0 });
    assert.equal((await list()).unread, 0);
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    process.env = saved;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  }
});
