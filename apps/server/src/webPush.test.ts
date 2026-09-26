import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { WebPushNotifications } from './webPush.js';

const subscription = {
  endpoint: 'https://fcm.googleapis.com/device-1',
  keys: { p256dh: 'public-material', auth: 'auth-material' }
};

function fixture(sendNotification: (...args: unknown[]) => Promise<unknown> = async () => ({})) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-web-push-'));
  const keys = join(directory, 'vapid.json');
  const subscriptions = join(directory, 'subscriptions.json');
  const sent: unknown[][] = [];
  const transport = {
    generateVAPIDKeys: () => ({ publicKey: 'public-key', privateKey: 'private-key' }),
    setVapidDetails: () => undefined,
    sendNotification: async (...args: unknown[]) => {
      sent.push(args);
      return sendNotification(...args);
    }
  };
  return {
    directory,
    keys,
    subscriptions,
    sent,
    service: new WebPushNotifications(keys, subscriptions, transport as never)
  };
}

test('VAPID keys are generated once and stored private', () => {
  const first = fixture();
  try {
    assert.equal(first.service.publicKey(), 'public-key');
    assert.equal(statSync(first.keys).mode & 0o777, 0o600);
    const second = new WebPushNotifications(first.keys, first.subscriptions, {
      generateVAPIDKeys: () => { throw new Error('must reuse keys'); },
      setVapidDetails: () => undefined,
      sendNotification: async () => ({})
    } as never);
    assert.equal(second.publicKey(), 'public-key');
  } finally {
    rmSync(first.directory, { recursive: true });
  }
});

test('activity delivery uses the bounded four-field payload and ignores non-waking events', async () => {
  const f = fixture();
  try {
    const registered = f.service.register(subscription, 'Phone');
    assert.equal(registered.count, 1);
    assert.equal(statSync(f.subscriptions).mode & 0o777, 0o600);
    await f.service.deliverActivity({
      id: 'chat:gah:s1:1:complete', occurredAt: '2026-09-25T12:00:00Z', profile: 'gah', sessionId: 's1',
      kind: 'chat_turn_completed', severity: 'success', title: 'gah: reply ready', message: 'Done.'
    });
    assert.equal(f.sent.length, 1);
    const payload = JSON.parse(f.sent[0][1] as string);
    assert.deepEqual(Object.keys(payload).sort(), ['body', 'id', 'title', 'url']);
    assert.equal(payload.url, '/?page=chat&profile=gah&chat=s1');
    assert.ok(Buffer.byteLength(f.sent[0][1] as string) < 3 * 1024);
    await f.service.deliverActivity({
      id: 'node-back', occurredAt: '2026-09-25T12:00:00Z', profile: null,
      kind: 'node_back', severity: 'success', title: 'Back', message: 'Back.'
    });
    assert.equal(f.sent.length, 1);
  } finally {
    rmSync(f.directory, { recursive: true });
  }
});

test('expired subscriptions are pruned and other delivery failures do not escape', async () => {
  let status = 410;
  const f = fixture(async () => { throw Object.assign(new Error('push failed'), { statusCode: status }); });
  try {
    f.service.register(subscription);
    f.service.register({ ...subscription, endpoint: 'https://fcm.googleapis.com/device-2' });
    const event = {
      id: 'failed', occurredAt: '2026-09-25T12:00:00Z', profile: 'gah', kind: 'dispatch_failed' as const,
      severity: 'error' as const, title: 'Failed', message: 'No secret details.'
    };
    await assert.doesNotReject(f.service.deliverActivity(event));
    assert.deepEqual(JSON.parse(readFileSync(f.subscriptions, 'utf8')), []);
    f.service.register(subscription);
    status = 500;
    await assert.doesNotReject(f.service.deliverActivity(event));
    assert.equal(f.service.list().count, 1);
  } finally {
    rmSync(f.directory, { recursive: true });
  }
});

test('revoking a paired device removes only its subscriptions', () => {
  const f = fixture();
  try {
    f.service.register(subscription, 'Phone', 'device-1');
    f.service.register({ ...subscription, endpoint: 'https://fcm.googleapis.com/device-2' }, 'Tablet', 'device-2');
    f.service.removeForDevice('device-1');
    const stored = JSON.parse(readFileSync(f.subscriptions, 'utf8')) as { deviceId?: string }[];
    assert.deepEqual(stored.map((entry) => entry.deviceId), ['device-2']);
  } finally {
    rmSync(f.directory, { recursive: true });
  }
});

test('pruning an expired send preserves a subscription registered in flight', async () => {
  let rejectDelivery: ((error: Error) => void) | undefined;
  const f = fixture(() => new Promise((_, reject) => { rejectDelivery = reject; }));
  try {
    f.service.register(subscription);
    const delivery = f.service.deliverActivity({
      id: 'failed', occurredAt: '2026-09-25T12:00:00Z', profile: 'gah', kind: 'dispatch_failed',
      severity: 'error', title: 'Failed', message: 'No secret details.'
    });
    await new Promise((resolve) => setImmediate(resolve));
    f.service.register({ ...subscription, endpoint: 'https://fcm.googleapis.com/device-2' });
    rejectDelivery?.(Object.assign(new Error('expired'), { statusCode: 410 }));
    await delivery;
    const stored = JSON.parse(readFileSync(f.subscriptions, 'utf8')) as { subscription: { endpoint: string } }[];
    assert.deepEqual(stored.map((entry) => entry.subscription.endpoint), ['https://fcm.googleapis.com/device-2']);
  } finally {
    rmSync(f.directory, { recursive: true });
  }
});

test('subscriptions accept only known push-service hosts and ignore unsafe stored endpoints', () => {
  const f = fixture();
  try {
    for (const endpoint of [
      'https://fcm.googleapis.com/device',
      'https://web.push.apple.com/device',
      'https://updates.push.services.mozilla.com/device',
      'https://wns.notify.windows.com/device'
    ]) f.service.register({ ...subscription, endpoint });
    assert.equal(f.service.list().count, 4);
    for (const endpoint of [
      'https://push.apple.com/device',
      'https://push.apple.com.attacker.test/device',
      'https://central-node.tailnet/device'
    ]) assert.throws(() => f.service.register({ ...subscription, endpoint }), /valid HTTPS push subscription/);

    writeFileSync(f.subscriptions, JSON.stringify([{
      id: 'unsafe', label: null, createdAt: new Date().toISOString(),
      subscription: { ...subscription, endpoint: 'https://central-node.tailnet/device' }
    }]));
    assert.deepEqual(f.service.list(), { count: 0 });
  } finally {
    rmSync(f.directory, { recursive: true });
  }
});
