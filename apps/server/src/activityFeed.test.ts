import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ActivityEvent, ControllerEvent } from '@git-agent-harness/contracts';
import { ActivityFeed, activitiesFromQuota, activityFromController, activityFromGateway, activityFromNode } from './activityFeed.js';
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

test('quota and gateway snapshots emit only actionable state', () => {
  const low = structuredClone(quotaFixture);
  low.candidates[0].quota_observations[0].quota_remaining_percent = 9;
  assert.equal(activitiesFromQuota(low as never)[0]?.kind, 'quota_near_limit');
  assert.deepEqual(activitiesFromQuota(quotaFixture as never), []);
  assert.equal(activityFromGateway({ degraded: true, lastError: 'recall timed out', lastFailedAt: 1, lastOkAt: null })?.kind, 'gateway_down');
  assert.equal(activityFromGateway({ degraded: false, lastError: null, lastFailedAt: null, lastOkAt: 1 }), null);
});
