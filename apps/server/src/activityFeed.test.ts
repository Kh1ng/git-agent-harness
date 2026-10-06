import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ActivityEvent, ControllerEvent, QuotaSnapshot } from '@git-agent-harness/contracts';
import { ActivityFeed, activitiesFromQuota, activityFromChat, activityFromController, activityFromGateway, activityFromNode } from './activityFeed.js';
import quotaFixture from '../tests/fixtures/gah/responses/quota.json' with { type: 'json' };

function controller(event_type: string, details: string): ControllerEvent {
  return {
    timestamp: '2026-09-12T12:00:00.000Z',
    event_type,
    profile: 'gah',
    work_id: '#941',
    details
  };
}

test('controller events become the requested high-signal notification kinds', () => {
  assert.equal(activityFromController(controller('observation_completed', 'profile=gah')), null);
  assert.equal(activityFromController(controller('dispatch_finished', 'dispatch_ticket: success'))?.kind, 'dispatch_completed');
  assert.equal(activityFromController(controller('dispatch_finished', 'dispatch_ticket: backend exited 1'))?.kind, 'dispatch_failed');
  assert.equal(activityFromController(controller('dispatch_finished', 'review_mr: success'))?.kind, 'review_ready');
  assert.equal(activityFromController(controller('backend_marked_unavailable', 'codex quota exhausted'))?.kind, 'quota_near_limit');
  assert.equal(activityFromController(controller('human_required', 'memory gateway recall failed'))?.kind, 'gateway_down');
});

test('activity replay is durable, cursor-based, and de-duplicated', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-activity-'));
  const path = join(directory, 'activity.jsonl');
  const first = activityFromNode({
    nodeId: 'worker-1',
    displayName: 'Mac worker',
    state: 'offline',
    occurredAt: '2026-09-12T12:00:00.000Z',
    message: 'Three failed checks.'
  });
  const second: ActivityEvent = {
    ...first,
    id: 'second',
    occurredAt: '2026-09-12T12:05:00.000Z',
    kind: 'node_back',
    severity: 'success',
    title: 'Mac worker is back online'
  };
  try {
    const feed = new ActivityFeed(path);
    assert.equal(feed.record(first), true);
    assert.equal(feed.record(first), false);
    assert.equal(feed.record(second), true);
    assert.deepEqual(feed.replay('gah', first.id), [second]);
    assert.deepEqual(new ActivityFeed(path).replay('gah', first.id), [second]);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('trimming replay history never makes an old event new again', () => {
  const delivered: string[] = [];
  const feed = new ActivityFeed(null, (event) => { delivered.push(event.id); });
  const event = (id: string): ActivityEvent => ({
    id,
    occurredAt: '2026-09-12T12:00:00.000Z',
    profile: 'gah',
    kind: 'dispatch_failed',
    severity: 'error',
    title: 'Work failed',
    message: id
  });
  assert.equal(feed.record(event('oldest')), true);
  for (let index = 0; index < 2_000; index++) assert.equal(feed.record(event(`new-${index}`)), true);
  assert.equal(feed.record(event('oldest')), false);
});

test('delivery failures never escape record', async () => {
  const feed = new ActivityFeed(null, async () => { throw new Error('offline push service'); });
  assert.equal(feed.record(activityFromNode({
    nodeId: 'worker-1', displayName: 'Mac worker', state: 'offline',
    occurredAt: '2026-09-12T12:00:00.000Z', message: 'Three failed checks.'
  })), true);
  await new Promise((resolve) => setImmediate(resolve));
});

test('silent backfill records without delivering old events', async () => {
  const delivered: string[] = [];
  const feed = new ActivityFeed(null, (event) => { delivered.push(event.id); });
  assert.equal(feed.record(activityFromNode({
    nodeId: 'worker-1', displayName: 'Mac worker', state: 'offline',
    occurredAt: '2026-09-12T12:00:00.000Z', message: 'Three failed checks.'
  }), false), true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(delivered, []);
});

test('quota and gateway snapshots emit only actionable state', () => {
  const low = structuredClone(quotaFixture);
  low.candidates[0].quota_observations[0].quota_remaining_percent = 9;
  assert.equal(activitiesFromQuota(low as never)[0]?.kind, 'quota_near_limit');
  assert.deepEqual(activitiesFromQuota(quotaFixture as never), []);
  assert.equal(activityFromGateway({ degraded: true, lastError: 'recall timed out', lastFailedAt: 1, lastOkAt: null })?.kind, 'gateway_down');
  assert.equal(activityFromGateway({ degraded: false, lastError: null, lastFailedAt: null, lastOkAt: 1 }), null);
});

/** #1336: an expired provider login must announce one action_required
 * event per failure streak, carrying the provider's remediation and how
 * long the streak has run -- never one per 30-minute marker. */
test('an auth_required quota source becomes one action_required event per streak', () => {
  const snapshot = structuredClone(quotaFixture) as unknown as QuotaSnapshot;
  snapshot.quota_checks = [
    {
      backend: 'claude',
      backend_instance: 'claude',
      provider: 'anthropic',
      checked_at: '2026-10-04T08:30:00Z',
      status: 'auth_required',
      failing_since: '2026-10-02T08:00:00Z',
      error: 'auth_required: Claude OAuth login expired; run claude auth login'
    },
    { backend: 'codex', checked_at: '2026-10-04T08:30:00Z', status: 'data' }
  ];
  const events = activitiesFromQuota(snapshot);
  assert.equal(events.length, 1);
  const event = events[0];
  assert.equal(event.kind, 'action_required');
  assert.equal(event.severity, 'warning');
  assert.equal(event.occurredAt, '2026-10-02T08:00:00Z');
  assert.ok(event.title.includes('claude'));
  assert.ok(event.title.includes('needs login'));
  assert.ok(event.message.includes('run claude auth login'));
  assert.ok(event.message.includes('2026-10-02T08:00:00.000Z'));

  // A later marker of the same streak keeps the id, so the feed dedupes it.
  const later = structuredClone(snapshot);
  later.quota_checks[0].checked_at = '2026-10-05T08:30:00Z';
  assert.equal(activitiesFromQuota(later)[0].id, event.id);

  // A successful check clears the condition: no event at all.
  const restored = structuredClone(snapshot);
  restored.quota_checks[0] = { backend: 'claude', backend_instance: 'claude', checked_at: '2026-10-05T09:00:00Z', status: 'data' };
  assert.deepEqual(activitiesFromQuota(restored), []);

  // A fresh failure after recovery is a new streak and announces again.
  const refailed = structuredClone(snapshot);
  refailed.quota_checks[0] = {
    backend: 'claude',
    backend_instance: 'claude',
    checked_at: '2026-10-06T08:00:00Z',
    status: 'auth_required',
    failing_since: '2026-10-06T08:00:00Z',
    error: 'auth_required: Claude OAuth login expired; run claude auth login'
  };
  const renewed = activitiesFromQuota(refailed)[0];
  assert.notEqual(renewed.id, event.id);
  assert.equal(renewed.occurredAt, '2026-10-06T08:00:00Z');

  // An unparsable streak start falls back to the check time instead of
  // throwing and dropping every other quota event.
  const malformed = structuredClone(refailed);
  malformed.quota_checks[0].failing_since = 'not a time';
  const fallback = activitiesFromQuota(malformed)[0];
  assert.equal(fallback.occurredAt, '2026-10-06T08:00:00Z');
});

test('live chat lifecycle maps only actionable outcomes with stable bounded content', () => {
  const base = { profile: 'gah', sessionId: 'session-1', turn: 3, occurredAt: '2026-09-25T12:00:00.000Z' } as const;
  assert.equal(activityFromChat({ ...base, phase: 'start' }), null);
  assert.equal(activityFromChat({ ...base, phase: 'end', outcome: 'cancelled' }), null);

  const complete = activityFromChat({ ...base, phase: 'end', outcome: 'complete', reply: `  hello\n${'x'.repeat(200)}` });
  assert.equal(complete?.id, 'chat:gah:session-1:3:chat_turn_completed');
  assert.equal(complete?.kind, 'chat_turn_completed');
  assert.equal(complete?.message.length, 120);
  assert.ok(!complete?.message.includes('\n'));

  const failed = activityFromChat({ ...base, phase: 'end', outcome: 'error', error: `token=${'x'.repeat(30)} failed` });
  assert.equal(failed?.id, 'chat:gah:session-1:3:chat_turn_failed');
  assert.equal(failed?.message, 'token=[REDACTED:SECRET] failed');

  const permission = activityFromChat({ ...base, phase: 'permission', permissionId: 'permission-1', tool: 'Run `secret command`' });
  assert.equal(permission?.kind, 'chat_permission_requested');
  assert.equal(permission?.message, 'Run requested');
  const secondPermission = activityFromChat({ ...base, phase: 'permission', permissionId: 'permission-2', tool: 'Edit file' });
  assert.notEqual(permission?.id, secondPermission?.id);
  const fallbackPermission = activityFromChat({ ...base, phase: 'permission', tool: 'Edit file' });
  const nextFallbackPermission = activityFromChat({
    ...base,
    phase: 'permission',
    tool: 'Edit file',
    occurredAt: '2026-09-25T12:00:01.000Z'
  });
  assert.notEqual(fallbackPermission?.id, nextFallbackPermission?.id);
});

const NOW = Date.parse('2026-09-30T12:00:00.000Z');
const DAY = 24 * 60 * 60 * 1000;
function feedEvent(id: string, kind: ActivityEvent['kind'], occurredAt: number): ActivityEvent {
  return { id, occurredAt: new Date(occurredAt).toISOString(), profile: 'gah', kind, severity: 'info', title: id, message: id };
}

test('a notification survives a flood of routine events for 30 days (#1273)', () => {
  const feed = new ActivityFeed(null, undefined, () => NOW);
  feed.record(feedEvent('ping-20d', 'review_ready', NOW - 20 * DAY), false);
  feed.record(feedEvent('ping-31d', 'review_ready', NOW - 31 * DAY), false);
  for (let index = 0; index < 5_000; index++) feed.record(feedEvent(`routine-${index}`, 'node_back', NOW), false);
  assert.deepEqual(feed.notifications().map((event) => event.id), ['ping-20d']);
  assert.equal(feed.replay('gah').length, 200);
});

test('opening one notification marks only it read; mark all is explicit (#1273)', () => {
  const feed = new ActivityFeed(null, undefined, () => NOW);
  const changes: string[] = [];
  feed.onChange((change) => changes.push(change.kind === 'unread' ? `unread:${change.count}` : `updated:${change.event.id}`));
  feed.record(feedEvent('a', 'review_ready', NOW - 2));
  feed.record(feedEvent('b', 'dispatch_failed', NOW - 1));
  feed.record(feedEvent('routine', 'node_back', NOW));
  feed.record(feedEvent('backfilled', 'dispatch_failed', NOW - 3), false);
  assert.equal(feed.unreadCount(), 2, 'a silently backfilled event never reached anyone');
  assert.equal(feed.markRead(['a', 'routine']), 1, 'routine events carry no read state');
  assert.deepEqual(feed.notifications().map((event) => [event.id, !!event.readAt]), [['b', false], ['a', true], ['backfilled', true]]);
  assert.equal(feed.markRead('all'), 1);
  assert.equal(feed.markRead('all'), 0);
  assert.deepEqual(changes, ['unread:1', 'unread:2', 'updated:a', 'unread:1', 'updated:b', 'unread:0']);
});

test('events recorded before read tracking load as read (#1273)', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-activity-legacy-'));
  const path = join(directory, 'activity.jsonl');
  try {
    writeFileSync(path, `${JSON.stringify(feedEvent('legacy', 'review_ready', NOW - DAY))}\n`);
    const feed = new ActivityFeed(path, undefined, () => NOW);
    assert.equal(feed.notifications().length, 1);
    assert.equal(feed.unreadCount(), 0);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('delivery receipts are stored on the event, announced, and survive a restart (#1273)', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-activity-receipts-'));
  const path = join(directory, 'activity.jsonl');
  try {
    const feed = new ActivityFeed(path, () => [
      { method: 'apns', target: 'iPhone', ok: true, at: '2026-09-30T12:00:01.000Z' },
      { method: 'channel', target: 'Telegram', ok: false, reason: 'HTTP 401', at: '2026-09-30T12:00:01.000Z' }
    ], () => NOW);
    const updated = new Promise<ActivityEvent>((resolve) => feed.onChange((change) => { if (change.kind === 'updated') resolve(change.event); }));
    feed.record(feedEvent('ping', 'node_offline', NOW));
    assert.deepEqual((await updated).deliveries?.map((receipt) => `${receipt.target}:${receipt.ok}`), ['iPhone:true', 'Telegram:false']);
    const reloaded = new ActivityFeed(path, undefined, () => NOW).notifications()[0];
    assert.equal(reloaded.deliveries?.[1].reason, 'HTTP 401');
    assert.equal(statSync(path).mode & 0o777, 0o600);
  } finally {
    rmSync(directory, { recursive: true });
  }
});
