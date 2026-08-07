// Issue #882: /api/claims/* route + authorization layer (registered node,
// declared profile). Real fake HTTP calls against a real createServer()
// instance, matching this repo's existing route-test convention
// (server.test.ts's withTestServer).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { createServer } from './server.js';
import { RegistryService } from './registryService.js';
import { ClaimsService } from './claimsService.js';
import { resetCachedCoordinatorIdentity, COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

function tempPath(prefix: string): string {
  return join(mkdtempSync(join(tmpdir(), prefix)), 'config.json');
}

async function withClaimsServer(
  setup: (registry: RegistryService) => void,
  testFn: (url: string) => Promise<void>
) {
  resetCachedCoordinatorIdentity();
  const registryService = new RegistryService(tempPath('gah-registry-'));
  const claimsService = new ClaimsService(tempPath('gah-claims-'));
  setup(registryService);

  const app = createServer({ registryService, claimsService });
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, resolve));
  const { port } = server.address() as AddressInfo;

  try {
    await testFn(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    resetCachedCoordinatorIdentity();
  }
}

let nextTestPort = 10000;

function registerTestNode(registry: RegistryService, nodeId: string, profiles: string[]) {
  registry.registerNode({
    node_id: nodeId,
    display_name: nodeId,
    advertised_url: `http://127.0.0.1:${nextTestPort++}`,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:UNUSED',
    profiles
  });
}

test('a registered node that declared the profile can acquire a claim', async () => {
  await withClaimsServer(
    (registry) => registerTestNode(registry, 'node-a', ['gah']),
    async (url) => {
      const res = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(res.status, 200);
      const body = (await res.json()) as { node_id: string };
      assert.equal(body.node_id, 'node-a');
    }
  );
});

test('an unregistered node_id is rejected, not silently granted a claim', async () => {
  await withClaimsServer(
    () => {},
    async (url) => {
      const res = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'ghost-node', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(res.status, 400);
    }
  );
});

test('a registered node cannot claim a profile it never declared', async () => {
  await withClaimsServer(
    (registry) => registerTestNode(registry, 'node-a', ['sportsball']),
    async (url) => {
      const res = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(res.status, 400);
      const body = (await res.json()) as { message: string };
      assert.match(body.message, /has not declared profile/);
    }
  );
});

test('a second node conflicting with an existing claim gets 409 with the holder identified', async () => {
  await withClaimsServer(
    (registry) => {
      registerTestNode(registry, 'node-a', ['gah']);
      registerTestNode(registry, 'node-b', ['gah']);
    },
    async (url) => {
      await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      const res = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-b', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(res.status, 409);
      const body = (await res.json()) as { held_by: { node_id: string } };
      assert.equal(body.held_by.node_id, 'node-a');
    }
  );
});

test('renew and release round-trip through the real routes', async () => {
  await withClaimsServer(
    (registry) => registerTestNode(registry, 'node-a', ['gah']),
    async (url) => {
      const acquireRes = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(acquireRes.status, 200);

      const renewRes = await fetch(`${url}/api/claims/renew`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(renewRes.status, 200);

      const releaseRes = await fetch(`${url}/api/claims/release`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(releaseRes.status, 200);

      // Released -- a different node can now acquire it.
      const reacquireRes = await fetch(`${url}/api/claims/acquire`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: 'ticket-1' })
      });
      assert.equal(reacquireRes.status, 200);
    }
  );
});

// Issue #882 / CodeQL js/missing-rate-limiting: these routes are
// authenticated but called frequently by design (renewals), so a generous
// per-IP limit exists as defense-in-depth against a buggy/compromised node
// rather than to throttle normal use -- this proves the limiter is
// actually wired in, not just configured and forgotten.
test('acquire is rate-limited past the configured per-IP window', async () => {
  await withClaimsServer(
    (registry) => registerTestNode(registry, 'node-a', ['gah']),
    async (url) => {
      let sawRateLimited = false;
      for (let i = 0; i < 65; i++) {
        const res = await fetch(`${url}/api/claims/acquire`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          // Distinct work_id per call -- isolating "rate limited" from
          // "conflicts with a claim this same loop already acquired".
          body: JSON.stringify({ node_id: 'node-a', profile: 'gah', work_id: `ticket-${i}` })
        });
        if (res.status === 429) {
          sawRateLimited = true;
          break;
        }
      }
      assert.ok(sawRateLimited, 'expected a 429 before 65 requests in one window');
    }
  );
});
