// Issue #882: central claim arbitration. Pure unit tests against
// ClaimsService directly (no HTTP) -- see claims.test.ts for the route +
// authorization layer on top of this.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { ClaimsService, ClaimConflictError } from './claimsService.js';

function tempClaimsPath(): string {
  return join(mkdtempSync(join(tmpdir(), 'gah-claims-test-')), 'claims-config.json');
}

test('a fresh claim is granted and reports the claiming node', () => {
  const service = new ClaimsService(tempClaimsPath());
  const lease = service.acquire('node-a', 'gah', 'ticket-1');
  assert.equal(lease.node_id, 'node-a');
  assert.equal(lease.profile, 'gah');
  assert.equal(lease.work_id, 'ticket-1');
  assert.ok(Date.parse(lease.expires_at) > Date.now());
});

test('a second node cannot acquire a claim already held by another node', () => {
  const service = new ClaimsService(tempClaimsPath());
  service.acquire('node-a', 'gah', 'ticket-1');
  assert.throws(() => service.acquire('node-b', 'gah', 'ticket-1'), ClaimConflictError);
});

test('the same node re-acquiring its own claim is idempotent, not a conflict', () => {
  const service = new ClaimsService(tempClaimsPath());
  const first = service.acquire('node-a', 'gah', 'ticket-1');
  const second = service.acquire('node-a', 'gah', 'ticket-1');
  assert.equal(second.node_id, 'node-a');
  assert.equal(second.claimed_at, first.claimed_at, 'claimed_at must not reset on idempotent re-acquire');
});

test('different profiles with the same work_id do not collide', () => {
  const service = new ClaimsService(tempClaimsPath());
  service.acquire('node-a', 'gah', 'ticket-1');
  // Must not throw -- distinct (profile, work_id) key.
  const lease = service.acquire('node-b', 'sportsball', 'ticket-1');
  assert.equal(lease.node_id, 'node-b');
});

test('renew extends expiry and requires holding the claim', () => {
  const service = new ClaimsService(tempClaimsPath());
  const acquired = service.acquire('node-a', 'gah', 'ticket-1', 60);
  const renewed = service.renew('node-a', 'gah', 'ticket-1', 120);
  assert.ok(Date.parse(renewed.expires_at) >= Date.parse(acquired.expires_at));
  assert.throws(() => service.renew('node-b', 'gah', 'ticket-1'), ClaimConflictError, 'a node that never held the claim cannot renew it');
});

test('renewing a nonexistent claim is a conflict, not a silent success', () => {
  const service = new ClaimsService(tempClaimsPath());
  assert.throws(() => service.renew('node-a', 'gah', 'never-claimed'), ClaimConflictError);
});

test('release is idempotent and a no-op for a claim you never held', () => {
  const service = new ClaimsService(tempClaimsPath());
  service.acquire('node-a', 'gah', 'ticket-1');
  service.release('node-b', 'gah', 'ticket-1'); // no-op, must not throw or steal
  assert.equal(service.getLease('gah', 'ticket-1')?.node_id, 'node-a');
  service.release('node-a', 'gah', 'ticket-1');
  assert.equal(service.getLease('gah', 'ticket-1'), undefined);
  service.release('node-a', 'gah', 'ticket-1'); // already gone, still must not throw
});

test('an expired claim is treated as available for a different node', () => {
  // The server clamps lease_seconds to a minimum, so a real acquire can't
  // produce an already-expired lease -- simulate the clock instead by
  // rewriting the persisted file's expires_at into the past directly.
  const path = tempClaimsPath();
  const service = new ClaimsService(path);
  service.acquire('node-a', 'gah', 'ticket-1', 60);

  const data = JSON.parse(readFileSync(path, 'utf8'));
  data.leases[0].expires_at = new Date(Date.now() - 1000).toISOString();
  writeFileSync(path, JSON.stringify(data));

  const reloaded = new ClaimsService(path);
  const lease = reloaded.acquire('node-b', 'gah', 'ticket-1');
  assert.equal(lease.node_id, 'node-b', 'an expired lease must not block a new acquire');
});

test('claims persist across ClaimsService instances backed by the same file', () => {
  const path = tempClaimsPath();
  const first = new ClaimsService(path);
  first.acquire('node-a', 'gah', 'ticket-1');

  const second = new ClaimsService(path);
  assert.equal(second.getLease('gah', 'ticket-1')?.node_id, 'node-a');
});
