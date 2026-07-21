import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { AvailabilityScope } from '@git-agent-harness/contracts';
import { aggregateProviderStatus } from './provider/ProviderRegistry.js';

function scope(eligible: boolean, model: string, reason: string | null = null): AvailabilityScope {
  return {
    backend: 'codex',
    model,
    eligible_now: eligible,
    reason,
    unavailable_until: null,
    source: null,
    last_error_summary: null,
    observed_at: '2026-07-21T00:00:00Z',
    scope: 'model_specific'
  };
}

test('provider availability is usable when any model scope is eligible', () => {
  const status = aggregateProviderStatus(
    [scope(false, 'blocked', 'quota_exhausted'), scope(true, 'healthy')],
    '1.0.0'
  );
  assert.deepEqual(status, { type: 'available', version: '1.0.0' });
});

test('provider availability reports failure when every route is blocked', () => {
  const status = aggregateProviderStatus(
    [scope(false, 'one', 'quota_exhausted'), scope(false, 'two', 'quota_exhausted')],
    '1.0.0'
  );
  assert.equal(status.type, 'error');
  assert.match(status.type === 'error' ? status.error : '', /All routes quota exhausted/);
});
