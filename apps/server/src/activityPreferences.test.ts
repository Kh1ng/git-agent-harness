import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES, notifiableActivity, type ActivityEvent, type ActivityKind } from '@git-agent-harness/contracts';
import { ActivityFeed } from './activityFeed.js';

const event = (id: string, kind: ActivityKind = 'node_offline', nodeState: ActivityEvent['nodeState'] = 'unreachable'): ActivityEvent => ({
  id, nodeId: 'worker', nodeState, occurredAt: new Date().toISOString(), profile: null,
  kind, severity: 'error', title: 'Worker activity', message: 'Three bad checks.'
});

test('optional health preferences persist while tasks, input requests and security keep notifying', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-activity-preferences-'));
  const path = join(root, 'activity.jsonl');
  const delivered: string[] = [];
  try {
    const feed = new ActivityFeed(path, (item) => { if (notifiableActivity(item)) delivered.push(item.id); });
    assert.deepEqual(feed.notificationPreferences(), DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES);
    feed.record(event('offline'));
    for (const kind of ['node_back', 'quota_near_limit', 'auth_restored'] as const) feed.record(event(kind, kind));
    await new Promise((done) => setImmediate(done));
    assert.deepEqual(delivered, ['offline']);
    feed.setNotificationPreferences({ nodeOffline: false, nodeBack: false, quotaNearLimit: false, authRestored: false });
    assert.equal(feed.unreadCount(), 0);
    assert.equal(feed.notifications().length, 0);
    assert.equal(statSync(`${path}.preferences.json`).mode & 0o777, 0o600);
    assert.throws(() => feed.setNotificationPreferences({ nodeOffline: 'false' }), /boolean/);
    const restored = new ActivityFeed(path, (item) => { if (notifiableActivity(item)) delivered.push(item.id); });
    assert.equal(restored.notificationPreferences().nodeOffline, false);
    restored.record(event('muted'));
    restored.record(event('security', 'node_offline', 'auth_failed'));
    restored.record(event('schema', 'node_offline', 'incompatible'));
    const attention: ActivityKind[] = ['dispatch_completed', 'review_ready', 'chat_turn_completed', 'dispatch_failed', 'chat_turn_failed', 'chat_permission_requested', 'action_required', 'gateway_down', 'auth_expired'];
    for (const kind of attention) restored.record(event(kind, kind));
    await new Promise((done) => setImmediate(done));
    assert.deepEqual(delivered.slice(1), ['security', 'schema', ...attention]);
    assert.equal(restored.replay('gah').length, 16, 'muted observations remain in the audit feed');
    assert.equal(notifiableActivity(restored.replay('gah').find(({ id }) => id === 'muted')!), false);
    restored.setNotificationPreferences({ nodeOffline: true, nodeBack: true, quotaNearLimit: true, authRestored: true });
    for (const kind of ['node_offline', 'node_back', 'quota_near_limit', 'auth_restored'] as const) restored.record(event(`enabled-${kind}`, kind));
    await new Promise((done) => setImmediate(done));
    assert.deepEqual(delivered.slice(-4), ['enabled-node_offline', 'enabled-node_back', 'enabled-quota_near_limit', 'enabled-auth_restored']);
    assert.equal(notifiableActivity(restored.replay('gah').find(({ id }) => id === 'muted')!), false, 'enabling future alerts never resurrects muted history');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('corrupt optional preference storage falls back to defaults without losing the control plane', () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-preferences-corrupt-'));
  const path = join(root, 'activity.jsonl');
  try {
    for (const contents of ['{broken', '{"nodeOffline":"no"}']) {
      writeFileSync(`${path}.preferences.json`, contents);
      const feed = new ActivityFeed(path);
      assert.deepEqual(feed.notificationPreferences(), DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES);
      feed.record(event('required', 'action_required'));
      assert.equal(feed.notifications()[0]?.id, 'required');
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('loading older unread observations persists suppression so enabling future alerts never resurrects history', () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-preference-migration-'));
  const path = join(root, 'activity.jsonl');
  try {
    writeFileSync(`${path}.preferences.json`, JSON.stringify({ ...DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES, nodeOffline: false }));
    writeFileSync(path, JSON.stringify({ ...event('old-offline'), readAt: null }) + '\n');
    const feed = new ActivityFeed(path);
    assert.equal(feed.unreadCount(), 0);
    feed.setNotificationPreferences(DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES);
    const restored = new ActivityFeed(path);
    assert.equal(restored.unreadCount(), 0);
    assert.equal(restored.notifications().length, 0);
    restored.setNotificationPreferences({ ...DEFAULT_ACTIVITY_NOTIFICATION_PREFERENCES, nodeOffline: false });
    restored.record({ ...event('unknown-provenance'), nodeState: undefined });
    assert.equal(restored.notifications().length, 1, 'unknown provenance remains enabled rather than hiding security');
  } finally { rmSync(root, { recursive: true, force: true }); }
});
