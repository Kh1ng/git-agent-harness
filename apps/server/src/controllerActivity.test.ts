import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ControllerEvent } from '@git-agent-harness/contracts';
import { deriveControllerActivity, deriveLastDecision } from './controllerActivity.js';

const event = (timestamp: string, event_type: string, details: string, reason_code: string | null = null): ControllerEvent => ({
  timestamp, event_type, details, reason_code, profile: 'gah', work_id: '#1381'
});

test('last decision is the newest decided or overridden action, with its reason', () => {
  const decision = deriveLastDecision([
    event('2026-10-05T10:00:00Z', 'action_decided', 'dispatch_ticket: ticket #1 is eligible'),
    event('2026-10-05T10:05:00Z', 'dispatch_finished', 'dispatch: success'),
    event('2026-10-05T10:06:00Z', 'action_decided',
      "human_required: MR on branch 'gah/x' classified NEEDS_FIX but fix retry cap (4) exceeded review_generation=abc",
      'fix_retry_cap_exceeded'),
  ]);
  assert.deepEqual(decision, {
    timestamp: '2026-10-05T10:06:00Z',
    kind: 'human_required',
    reason: "MR on branch 'gah/x' classified NEEDS_FIX but fix retry cap (4) exceeded",
    work_id: '#1381',
    reason_code: 'fix_retry_cap_exceeded'
  });

  const overridden = deriveLastDecision([
    event('2026-10-05T10:06:00Z', 'action_decided', 'fix_mr: reuse branch'),
    event('2026-10-05T10:06:00Z', 'action_overridden', 'fix_mr -> human_required: stuck loop detected'),
  ]);
  assert.equal(overridden?.kind, 'human_required');
  assert.equal(overridden?.reason, 'stuck loop detected');

  assert.equal(deriveLastDecision([]), null);
});

const runEvent = (fields: Partial<ControllerEvent>): ControllerEvent => ({
  timestamp: '2026-10-06T12:00:00Z', event_type: 'dispatch_started', profile: 'gah', work_id: '#7', run_id: 'run-1', details: 'review: gah/job-7', ...fields
});

test('a run that ended on routing carries the routes the router passed over', () => {
  const skipped = [{ backend: 'agy', backend_instance: 'agy:google-native', model: 'Gemini 3.1 Pro (High)', reason: 'max_concurrent_reached', unavailable_until: null }];
  const [run] = deriveControllerActivity([
    runEvent({}),
    runEvent({ timestamp: '2026-10-06T12:00:03Z', event_type: 'dispatch_finished', details: 'review: deferred_capacity: no eligible backend', skipped })
  ]);
  assert.equal(run.status, 'failed');
  assert.deepEqual(run.skipped, skipped);
});

test('a run from an event without the list has none', () => {
  const [run] = deriveControllerActivity([
    runEvent({}),
    runEvent({ timestamp: '2026-10-06T12:00:03Z', event_type: 'dispatch_finished', details: 'review: success' })
  ]);
  assert.equal(run.status, 'finished');
  assert.equal(run.skipped, undefined);
});
