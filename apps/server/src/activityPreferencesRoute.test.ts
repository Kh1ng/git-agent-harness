import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES } from '@git-agent-harness/contracts';
import { ActivityFeed } from './activityFeed.js';
import { DeviceAccess } from './deviceAccess.js';
import { RegistryService } from './registryService.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { createServer } from './server.js';

test('notification preferences require authentication, allow a paired phone, and save idempotently', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-preference-route-'));
  const saved = { ...process.env };
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'identity.json');
  resetCachedCoordinatorIdentity();
  const access = new DeviceAccess(join(root, 'devices.json'));
  const feed = new ActivityFeed(join(root, 'activity.jsonl'));
  const server = http.createServer(createServer({ node: { role: 'central', central_url: null }, activityFeed: feed, deviceAccess: access, registryService: new RegistryService(join(root, 'registry.json')) }));
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const path = origin + '/api/activity/notification-preferences';
  const offer = access.create({ id: 'central', name: 'Central', origin });
  const { token } = access.redeem(offer.code, 'central', origin, 'Phone');
  const headers = { Origin: origin, 'X-Forwarded-For': '198.51.100.1', Cookie: `gah_device=${token}`, 'Content-Type': 'application/json' };
  const post = (body: object, custom = headers) => fetch(path, { method: 'POST', headers: custom, body: JSON.stringify(body) });
  const next = { nodeOffline: false, nodeBack: true, quotaNearLimit: false, authRestored: false };
  try {
    assert.equal((await fetch(path, { headers: { 'X-Forwarded-For': '198.51.100.1' } })).status, 401);
    assert.equal((await post(next, { ...headers, Cookie: '' })).status, 401);
    assert.equal((await post(next, { ...headers, Origin: 'http://untrusted.test' })).status, 401);
    assert.deepEqual(await (await fetch(path, { headers })).json(), DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES);
    assert.equal((await post({ ...next, nodeOffline: 'false' })).status, 400);
    assert.equal((await post({ ...next, dispatchCompleted: false })).status, 400, 'core attention cannot be switched off');
    for (let count = 0; count < 2; count++) {
      const response = await post(next);
      assert.equal(response.status, 200);
      assert.deepEqual(await response.json(), next);
    }
    assert.deepEqual(new ActivityFeed(join(root, 'activity.jsonl')).notificationPreferences(), next);
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    process.env = saved;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  }
});
