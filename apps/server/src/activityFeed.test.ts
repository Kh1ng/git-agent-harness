import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ActivityEvent, ControllerEvent } from '@git-agent-harness/contracts';
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
