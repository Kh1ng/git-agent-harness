// Issue #885: ticket-scoped memory interface. Uses a real fake HTTP gateway
// (not a mocked fetch) matching this repo's existing test convention
// (server.test.ts spins up a real http.Server rather than mocking fetch).
// Extended for issue #878: the gateway is fail-open -- recall/capture/flush
// never throw, they degrade with a visible flag -- and a profile that has
// opted out skips the gateway entirely.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { existsSync, rmSync } from 'node:fs';

import {
  normalizeRemoteUrl,
  sessionKeyForTicket,
  recallForTicket,
  captureForTicket,
  recall,
  capture,
  flushSession,
  gatewayHealth
} from './managerChat/memoryGatewayClient.js';

/** A fake gateway backed by an in-memory map keyed on session_key, so
 * capture-then-recall round-trips through the same store a real gateway
 * would use, and namespace isolation between keys is genuinely exercised
 * rather than assumed. */
async function withFakeGateway(testFn: (baseUrl: string) => Promise<void>): Promise<void> {
  const store = new Map<string, string[]>();
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      const payload = JSON.parse(body || '{}');
      const sessionKey = payload.session_key as string;
      if (req.url === '/capture') {
        const entries = store.get(sessionKey) ?? [];
        entries.push(`${payload.user_content}\n${payload.assistant_content}`);
        store.set(sessionKey, entries);
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
        return;
      }
      if (req.url === '/recall') {
        const entries = store.get(sessionKey) ?? [];
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(
          JSON.stringify({
            context: entries.join('\n---\n'),
            memory_count: entries.length,
            code: 0,
            message: 'ok'
          })
        );
        return;
      }
      res.writeHead(404);
      res.end();
    });
  });
  await new Promise<void>((resolvePromise) => server.listen(0, resolvePromise));
  const { port } = server.address() as AddressInfo;

  const saved = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${port}`;
  try {
    await testFn(`http://127.0.0.1:${port}`);
  } finally {
    if (saved === undefined) delete process.env.TDAI_GATEWAY_URL;
    else process.env.TDAI_GATEWAY_URL = saved;
    await new Promise<void>((resolvePromise) => server.close(() => resolvePromise()));
  }
}

test('sessionKeyForTicket normalizes ticket id the same way #71/71/TICKET-071 all mean the same ticket', async () => {
  // profile "does-not-exist" has no local checkout to resolve a git remote
  // for, so resolveProjectKey falls back to the raw profile name -- fine,
  // this test is only about ticket-id normalization, not project keying.
  const a = await sessionKeyForTicket('does-not-exist', '#71');
  const b = await sessionKeyForTicket('does-not-exist', '71');
  const c = await sessionKeyForTicket('does-not-exist', 'TICKET-071');

  assert.equal(a, b);
  assert.equal(b, c);
  assert.match(a, /:#71$/);
});

test('captureForTicket then recallForTicket round-trips through the same ticket-scoped key', async () => {
  await withFakeGateway(async () => {
    await captureForTicket('does-not-exist', '#42', 'what should we do', 'use the flat key scheme');
    const result = await recallForTicket('does-not-exist', '42', 'flat key scheme');

    assert.equal(result.memoryCount, 1);
    assert.match(result.context, /use the flat key scheme/);
  });
});

test('a different ticket under the same profile does not see another ticket\'s captured content', async () => {
  await withFakeGateway(async () => {
    await captureForTicket('does-not-exist', '#1', 'ticket one context', 'ticket one answer');
    const otherTicket = await recallForTicket('does-not-exist', '#2', 'anything');

    assert.equal(otherTicket.memoryCount, 0);
    assert.equal(otherTicket.context, '');
  });
});

test('the same ticket under a different profile does not see another profile\'s captured content', async () => {
  await withFakeGateway(async () => {
    await captureForTicket('profile-a', '#1', 'profile a context', 'profile a answer');
    const otherProfile = await recallForTicket('profile-b', '#1', 'anything');

    assert.equal(otherProfile.memoryCount, 0);
  });
});

test('normalizeRemoteUrl is unaffected by these changes (regression guard)', () => {
  assert.equal(
    normalizeRemoteUrl('https://github.com/Kh1ng/git-agent-harness.git'),
    'github.com/kh1ng/git-agent-harness'
  );
});

// ── #878 fail-open ──────────────────────────────────────────────────────────

/** Point the gateway URL at a port with nothing listening, so every call
 * fails at the transport level the way a down gateway would. */
async function withUnreachableGateway(testFn: () => Promise<void>): Promise<void> {
  const saved = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:1';
  try {
    await testFn();
  } finally {
    if (saved === undefined) delete process.env.TDAI_GATEWAY_URL;
    else process.env.TDAI_GATEWAY_URL = saved;
  }
}

test('an unreachable gateway makes recall degrade, not throw', async () => {
  await withUnreachableGateway(async () => {
    const result = await recall('does-not-exist', 'anything');
    assert.equal(result.context, '');
    assert.equal(result.memoryCount, 0);
    assert.equal(result.degraded, true);
    assert.ok(result.error, 'degraded result carries the failure reason');
    assert.equal(gatewayHealth().degraded, true, 'health snapshot reflects the failure');
  });
});

test('an unreachable gateway makes capture degrade, not throw', async () => {
  await withUnreachableGateway(async () => {
    const result = await capture('does-not-exist', 'user', 'assistant');
    assert.equal(result.l0Recorded, 0);
    assert.equal(result.degraded, true);
  });
});

test('an unreachable gateway makes flushSession report false, not throw', async () => {
  await withUnreachableGateway(async () => {
    assert.equal(await flushSession('does-not-exist'), false);
    assert.equal(gatewayHealth().degraded, true);
  });
});

test('a profile that opted out of the gateway skips it entirely: no calls, no degradation', async () => {
  const settingsPath = join(tmpdir(), `gah-gateway-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = settingsPath;
  const savedUrl = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:1'; // would fail if actually called
  try {
    // Opt this profile out via the gateway settings store.
    const { writeGatewaySettings } = await import('./gatewaySettingsStore.js');
    writeGatewaySettings({ disabledProfiles: ['opted-out-profile'] });

    const recalled = await recall('opted-out-profile', 'anything');
    assert.deepEqual(recalled, { context: '', memoryCount: 0 }, 'opt-out recall is a clean empty result');
    assert.equal(recalled.degraded, undefined, 'no degradation flag: the gateway was never called');

    const captured = await capture('opted-out-profile', 'u', 'a');
    assert.deepEqual(captured, { l0Recorded: 0 });
    assert.equal(captured.degraded, undefined);

    assert.equal(await flushSession('opted-out-profile'), false);

    // A profile that did NOT opt out still hits the gateway and degrades --
    // proving the opt-out (not the dead URL) is what kept the result clean.
    const recalledActive = await recall('active-profile', 'anything');
    assert.equal(recalledActive.degraded, true);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (savedUrl === undefined) delete process.env.TDAI_GATEWAY_URL;
    else process.env.TDAI_GATEWAY_URL = savedUrl;
    if (existsSync(settingsPath)) rmSync(settingsPath);
  }
});

test('a working gateway clears the degraded health snapshot', async () => {
  await withFakeGateway(async () => {
    await captureForTicket('does-not-exist', '#1', 'a', 'b');
    await recallForTicket('does-not-exist', '#1', 'a');
    assert.equal(gatewayHealth().degraded, false);
    assert.equal(gatewayHealth().lastError, null);
  });
});
