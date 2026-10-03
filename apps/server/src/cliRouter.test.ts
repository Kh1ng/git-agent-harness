import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { mutationSafety } from './mutationSafety.js';
import { createCliRouterQuotaObserver, cliRouterRouter, validateRouterUrl, boundedUpstreamFetch, readSettings, writeSettings } from './cliRouter.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import type { CliRouterSnapshot, CliRouterStoredSettings } from '@git-agent-harness/contracts';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMockFetch(responses: Record<string, { status: number; body: unknown }>) {
  return async (url: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const urlStr = typeof url === 'string' ? url : url instanceof URL ? url.toString() : url.url;
    if (init?.redirect === 'error') {
      // Check for redirect responses in mocks
      for (const [pattern, mock] of Object.entries(responses)) {
        if (urlStr.includes(pattern) && mock.status >= 300 && mock.status < 400) {
          throw new TypeError('redirect mode is set to error');
        }
      }
    }
    for (const [pattern, mock] of Object.entries(responses)) {
      if (urlStr.includes(pattern)) {
        const body = JSON.stringify(mock.body);
        return new Response(body, { status: mock.status, headers: { 'Content-Type': 'application/json' } });
      }
    }
    return new Response('Not Found', { status: 404 });
  };
}

interface TestServerOptions {
  settings?: CliRouterStoredSettings | null;
  mockFetch?: typeof globalThis.fetch;
  withDeviceAccess?: boolean;
  autoRefresh?: boolean;
  now?: () => number;
  recordQuota?: (record: Record<string, unknown>) => Promise<void>;
}

async function createTestServer(opts: TestServerOptions = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-cli-router-test-'));
  let currentSettings = opts.settings ?? null;
  const readFn = () => currentSettings;
  const writeFn = (s: CliRouterStoredSettings) => { currentSettings = s; };

  const app = express();
  const access = opts.withDeviceAccess ? new DeviceAccess(join(directory, 'devices.json')) : undefined;
  if (access) app.locals.deviceAccess = access;
  app.use(express.json());
  app.use(rateLimit({ windowMs: 60_000, limit: 200, validate: false }));
  app.use('/api', authMiddleware);
  const mutation = mutationSafety('cli-router-test', directory);
  app.use('/api/cli-router', cliRouterRouter(mutation, {
    fetchFn: opts.mockFetch,
    readSettingsFn: readFn,
    writeSettingsFn: writeFn,
    autoRefresh: opts.autoRefresh ?? false,
    now: opts.now,
    recordQuotaFn: opts.recordQuota ?? (async () => {}),
  }));

  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  const base = `http://127.0.0.1:${port}`;

  return {
    base,
    server,
    directory,
    access,
    get currentSettings() { return currentSettings; },
    cleanup: async () => {
      await new Promise<void>(resolve => server.close(() => resolve()));
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

function get(base: string, path: string) {
  return fetch(`${base}/api/cli-router${path}`);
}

function mutation(base: string, method: string, path: string, body: object, key?: string) {
  return fetch(`${base}/api/cli-router${path}`, {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...(key ? { 'Idempotency-Key': key } : {}),
    },
    body: JSON.stringify(body),
  });
}

let keyCounter = 0;
function freshKey() { return `test-key-${Date.now()}-${++keyCounter}`.padEnd(16, '0'); }

test('read-only controller snapshots automatically collect distinct AGY pools, persist them, and throttle concurrent reads', async () => {
  let now = Date.now();
  const records: Record<string, unknown>[] = [];
  const accounts = [file({ id: 'agy1', provider: 'antigravity', project_id: 'p1' }), file({ id: 'agy2', auth_index: 'idx2', provider: 'antigravity', project_id: 'p2' })];
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: accounts } };
    if (c.url.endsWith('/models')) return { status: 200, body: { data: [] } };
    if (c.url.endsWith('/config')) return { status: 200, body: { routing: { strategy: 'round-robin', 'session-affinity': true } } };
    if (c.url.endsWith('/api-call')) return { status: 200, body: { status_code: 200, body: { groups: [{ displayName: 'Gemini', buckets: [{ bucketId: 'gemini-weekly', window: 'weekly', remainingFraction: c.body.auth_index === 'idx2' ? .54 : 0, resetTime: '2026-10-09T05:00:00Z' }] }, { displayName: 'Claude and GPT', buckets: [{ bucketId: '3p-weekly', window: 'weekly', remainingFraction: 0, resetTime: '2026-10-09T04:00:00Z' }] }] } } };
  });
  const ctx = await createTestServer({
    settings: { url: 'https://auto.example.com', apiKey: 'k', managementKey: 'm', accountBackends: { agy1: 'agy', agy2: 'agy-second' } },
    mockFetch: rec.fn, autoRefresh: true, now: () => now,
    recordQuota: async record => { records.push(record); },
  });
  try {
    await Promise.all([get(ctx.base, '/'), get(ctx.base, '/')]);
    // Background observation must not hold the initial dashboard request open.
    for (let i = 0; i < 40 && records.length < 4; i++) await new Promise(resolve => setTimeout(resolve, 5));
    const snapshot = await (await get(ctx.base, '/')).json() as CliRouterSnapshot;
    assert.deepEqual(snapshot.accounts.map(a => a.quotas.map(q => q.remainingPercent)), [[0, 0], [54, 0]]);
    assert.equal(rec.calls.filter(c => c.url.endsWith('/api-call')).length, 2);
    assert.deepEqual(records.map(r => [r.backend, r.backend_instance, r.quota_pool, r.quota_window, r.quota_remaining_percent]), [
      ['agy', 'agy:google-native', 'agy:google-native', 'weekly', 0],
      ['agy', 'agy:external', 'agy:external', 'weekly', 0],
      ['agy-second', 'agy-second:google-native', 'agy-second:google-native', 'weekly', 54],
      ['agy-second', 'agy-second:external', 'agy-second:external', 'weekly', 0],
    ]);
    now += 15 * 60_000 + 1;
    await get(ctx.base, '/');
    for (let i = 0; i < 40 && records.length < 8; i++) await new Promise(resolve => setTimeout(resolve, 5));
    assert.equal(records.length, 8, 'only stale observations refresh');
  } finally { await ctx.cleanup(); }
});

test('a partial router outage still observes accounts and failure checks never reuse previous balances', async () => {
  let now = Date.now();
  let state: 'good' | 'empty' | 'failed' = 'good';
  const records: Record<string, unknown>[] = [];
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [file({ id: 'partial', provider: 'antigravity', project_id: 'p' })] } };
    if (c.url.endsWith('/models')) return { status: 503, body: {} };
    if (c.url.endsWith('/config')) return { status: 200, body: { routing: { strategy: 'round-robin' } } };
    if (c.url.endsWith('/api-call')) return { status: 200, body: { status_code: state === 'failed' ? 401 : 200, body: { groups: state === 'empty' ? [] : [{ displayName: 'Gemini', buckets: [{ bucketId: 'gemini-weekly', window: 'weekly', remainingFraction: .5, resetTime: '2026-10-09T05:00:00Z' }] }] } } };
  });
  const ctx = await createTestServer({
    settings: { url: 'https://partial.example.com', apiKey: 'k', managementKey: 'm', accountBackends: { partial: 'agy' } },
    mockFetch: rec.fn, autoRefresh: true, now: () => now, recordQuota: async record => { records.push(record); },
  });
  try {
    const first = await (await get(ctx.base, '/')).json() as CliRouterSnapshot;
    assert.equal(first.status, 'unavailable');
    assert.equal(first.accounts.length, 1, 'model inventory failure cannot erase credential inventory');
    for (let i = 0; i < 40 && records.length < 1; i++) await new Promise(resolve => setTimeout(resolve, 5));
    assert.equal(records[0].quota_remaining_percent, 50);
    for (const failure of ['empty', 'failed'] as const) {
      state = failure; now += 15 * 60_000 + 1;
      const count = records.length;
      await get(ctx.base, '/');
      for (let i = 0; i < 40 && records.length < count + 2; i++) await new Promise(resolve => setTimeout(resolve, 5));
      assert.deepEqual(records.slice(count).map(check => check.backend_instance), ['agy:google-native', 'agy:external']);
      for (const check of records.slice(count)) {
        assert.equal(check.quota_remaining_percent, undefined);
        assert.equal(check.quota_window, undefined);
        assert.equal(check.observed_at, undefined);
        assert.ok(check.backend_instance === 'agy:google-native' || check.backend_instance === 'agy:external');
        if (failure === 'failed') assert.equal(check.check_error, 'Failed to refresh quota from provider');
      }
      assert.ok(records.length > count);
    }
  } finally { await ctx.cleanup(); }
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test('GET /api/cli-router returns unconfigured snapshot when no settings', async () => {
  const ctx = await createTestServer();
  try {
    const res = await get(ctx.base, '/');
    assert.equal(res.status, 200);
    const snapshot = await res.json() as CliRouterSnapshot;
    assert.equal(snapshot.status, 'unconfigured');
    assert.equal(snapshot.settings.url, null);
    assert.equal(snapshot.settings.hasApiKey, false);
    assert.equal(snapshot.settings.hasManagementKey, false);
    assert.deepStrictEqual(snapshot.accounts, []);
    assert.deepStrictEqual(snapshot.models, []);
  } finally {
    await ctx.cleanup();
  }
});

test('GET /api/cli-router returns connected snapshot with upstream data', async () => {
  const mockFetch = makeMockFetch({
    '/v1/models': { status: 200, body: { data: [{ id: 'gpt-4', owned_by: 'openai' }] } },
    '/v0/management/auth-files': { status: 200, body: { files: [{ id: 'acc-1', name: 'test', auth_index: 0, provider: 'claude', type: 'oauth', label: 'My Claude', disabled: false, unavailable: false, next_retry_after: null, project_id: null, quota: null, model_quotas: null }] } },
    '/v0/management/config': { status: 200, body: { routing: { strategy: 'fill-first', 'session-affinity': true } } },
  });

  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'test-api-key', managementKey: 'test-mgmt-key' },
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await get(ctx.base, '/');
    assert.equal(res.status, 200);
    const snapshot = await res.json() as CliRouterSnapshot;
    assert.equal(snapshot.status, 'connected');
    assert.equal(snapshot.strategy, 'fill-first');
    assert.equal(snapshot.sessionAffinity, true);
    assert.equal(snapshot.accounts.length, 1);
    assert.equal(snapshot.accounts[0].id, 'acc-1');
    assert.equal(snapshot.accounts[0].provider, 'claude');
    assert.equal(snapshot.models.length, 1);
    assert.equal(snapshot.models[0].id, 'gpt-4');
  } finally {
    await ctx.cleanup();
  }
});

test('secret redaction: API keys never appear in responses or settings view', async () => {
  const mockFetch = makeMockFetch({
    '/v1/models': { status: 200, body: { data: [] } },
    '/v0/management/auth-files': { status: 200, body: { files: [] } },
    '/v0/management/config': { status: 200, body: {} },
  });

  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'super-secret-api-key-12345', managementKey: 'ultra-secret-mgmt-key-67890' },
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await get(ctx.base, '/');
    const text = await res.text();
    assert.ok(!text.includes('super-secret-api-key-12345'), 'API key must not appear in response');
    assert.ok(!text.includes('ultra-secret-mgmt-key-67890'), 'Management key must not appear in response');
    const snapshot = JSON.parse(text) as CliRouterSnapshot;
    assert.equal(snapshot.settings.hasApiKey, true);
    assert.equal(snapshot.settings.hasManagementKey, true);
    // Verify no key fields exist on settings object
    assert.equal(Object.keys(snapshot.settings).sort().join(','), 'hasApiKey,hasManagementKey,url');
  } finally {
    await ctx.cleanup();
  }
});

test('redirect refusal: upstream redirects are rejected', async () => {
  // boundedUpstreamFetch uses redirect: 'error', we test it directly
  let threw = false;
  try {
    await boundedUpstreamFetch({
      url: 'http://127.0.0.1:1/redirect-target',
      fetchFn: async (_url, init) => {
        if (init?.redirect === 'error') {
          // Simulate the fetch API behavior with redirect: 'error' when encountering a redirect
          throw new TypeError('Failed to fetch: redirect mode is set to error');
        }
        return new Response('', { status: 302, headers: { Location: 'http://evil.example.com' } });
      },
    });
  } catch (err) {
    threw = true;
    assert.ok(err instanceof TypeError || err instanceof Error);
  }
  assert.ok(threw, 'redirect: error must cause fetch to throw');
});

test('corruption preservation: invalid/partial settings file returns unconfigured, not zero data', async () => {
  // Test with corrupted settings
  const ctx = await createTestServer({
    settings: null, // Simulates unreadable/missing file
  });
  try {
    const res = await get(ctx.base, '/');
    const snapshot = await res.json() as CliRouterSnapshot;
    // Must return unconfigured rather than pretending keys are empty
    assert.equal(snapshot.status, 'unconfigured');
    assert.equal(snapshot.settings.hasApiKey, false);
    assert.equal(snapshot.settings.hasManagementKey, false);
    assert.deepStrictEqual(snapshot.accounts, []);
    assert.deepStrictEqual(snapshot.models, []);
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: PUT settings rejects unknown keys', async () => {
  const ctx = await createTestServer();
  try {
    const res = await mutation(ctx.base, 'PUT', '/settings', {
      url: 'https://proxy.example.com',
      apiKey: 'key1',
      managementKey: 'key2',
      evilField: 'injected',
    }, freshKey());
    assert.equal(res.status, 400);
    const body = await res.json() as { error: string };
    assert.equal(body.error, 'invalid_request');
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: PUT settings rejects URLs with path/query/hash', async () => {
  const ctx = await createTestServer();
  try {
    for (const url of [
      'https://proxy.example.com/v1',
      'https://proxy.example.com?token=abc',
      'https://proxy.example.com#section',
      'https://user:pass@proxy.example.com',
      'http://remote-host.example.com', // non-loopback HTTP
    ]) {
      const res = await mutation(ctx.base, 'PUT', '/settings', {
        url,
        apiKey: 'key1',
        managementKey: 'key2',
      }, freshKey());
      assert.ok(res.status === 400 || res.status === 409, `Expected 400 for URL ${url}, got ${res.status}`);
    }
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: POST routing rejects unknown strategy', async () => {
  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/routing', {
      strategy: 'random-chaos',
      sessionAffinity: true,
    }, freshKey());
    assert.equal(res.status, 400);
    const body = await res.json() as { error: string };
    assert.equal(body.error, 'invalid_strategy');
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: POST routing rejects extra keys', async () => {
  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/routing', {
      strategy: 'round-robin',
      sessionAffinity: true,
      extraField: 'nope',
    }, freshKey());
    assert.equal(res.status, 400);
    const body = await res.json() as { error: string };
    assert.equal(body.error, 'invalid_request');
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: POST accounts/status rejects non-boolean disabled', async () => {
  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/status', {
      id: 'acc-1',
      disabled: 'yes',
    }, freshKey());
    assert.equal(res.status, 400);
  } finally {
    await ctx.cleanup();
  }
});

test('mutation validation: POST accounts/refresh fails for unknown id', async () => {
  const mockFetch = makeMockFetch({
    '/v0/management/auth-files': { status: 200, body: { files: [{ id: 'known-1', name: 'ok', auth_index: 0, provider: 'claude', type: 'oauth', label: 'x', disabled: false, unavailable: false, next_retry_after: null, project_id: null, quota: null, model_quotas: null }] } },
    '/v1/models': { status: 200, body: { data: [] } },
    '/v0/management/config': { status: 200, body: {} },
  });
  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/refresh', {
      id: 'nonexistent-account',
    }, freshKey());
    assert.equal(res.status, 404);
    const body = await res.json() as { error: string };
    assert.equal(body.error, 'account_not_found');
  } finally {
    await ctx.cleanup();
  }
});

test('auth: mutations require idempotency key', async () => {
  const ctx = await createTestServer();
  try {
    // Missing idempotency key
    const res = await mutation(ctx.base, 'PUT', '/settings', {
      url: 'https://proxy.example.com',
      apiKey: 'k',
      managementKey: 'k',
    });
    assert.equal(res.status, 400);
    const body = await res.json() as { error: string };
    assert.equal(body.error, 'idempotency_key_required');
  } finally {
    await ctx.cleanup();
  }
});

test('auth: paired devices cannot configure CLI router (owner required)', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-cli-router-auth-'));
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const app = express();
  app.locals.deviceAccess = access;
  app.use(express.json());
  app.use(rateLimit({ windowMs: 60_000, limit: 200, validate: false }));
  app.use('/api', authMiddleware);
  const mut = mutationSafety('cli-router-auth-test', directory);
  app.use('/api/cli-router', cliRouterRouter(mut, {
    fetchFn: makeMockFetch({
      '/v1/models': { status: 200, body: { data: [] } },
      '/v0/management/auth-files': { status: 200, body: { files: [] } },
      '/v0/management/config': { status: 200, body: {} },
    }) as typeof globalThis.fetch,
    readSettingsFn: () => null,
    writeSettingsFn: () => {},
  }));

  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  const origin = `http://127.0.0.1:${port}`;

  try {
    // Device can read
    const offer = access.create({ id: 'cli-router-auth-test', name: 'Test', origin });
    const paired = access.redeem(offer.code, 'cli-router-auth-test', origin, 'TestDevice');

    const readRes = await fetch(`${origin}/api/cli-router/`, {
      headers: { Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}` },
    });
    assert.equal(readRes.status, 200, 'Paired device can read snapshot');

    // Device cannot write
    const writeRes = await fetch(`${origin}/api/cli-router/settings`, {
      method: 'PUT',
      headers: {
        'Content-Type': 'application/json',
        'Idempotency-Key': freshKey(),
        Origin: origin,
        Cookie: `${DEVICE_COOKIE}=${paired.token}`,
      },
      body: JSON.stringify({ url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' }),
    });
    assert.equal(writeRes.status, 403, 'Paired device cannot configure CLI router');
  } finally {
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test('URL validation: accepts valid URLs, rejects invalid ones', () => {
  // Valid
  assert.ok(validateRouterUrl('https://proxy.example.com'));
  assert.ok(validateRouterUrl('http://127.0.0.1:8080'));
  assert.ok(validateRouterUrl('https://my-proxy.internal:443'));

  // Invalid
  const invalid = [
    'ftp://proxy.example.com',
    'https://user:pass@proxy.example.com',
    'https://proxy.example.com/path',
    'https://proxy.example.com?q=1',
    'https://proxy.example.com#hash',
    'http://remote-server.com', // non-loopback HTTP
    'not-a-url',
  ];
  for (const url of invalid) {
    assert.throws(() => validateRouterUrl(url), `Expected ${url} to be rejected`);
  }
});

test('bounded upstream fetch respects size cap', async () => {
  // Create a response larger than MAX_RESPONSE_BYTES (512 KiB)
  const hugeBody = 'x'.repeat(600 * 1024);
  let threw = false;
  try {
    await boundedUpstreamFetch({
      url: 'http://test.example.com/huge',
      fetchFn: async () => new Response(hugeBody, { status: 200 }),
    });
  } catch (err) {
    threw = true;
    assert.ok(err instanceof Error);
    assert.ok(err.message.includes('exceeded'));
  }
  assert.ok(threw, 'Must throw on oversized response');
});

test('PUT settings tests upstream connectivity before saving', async () => {
  const mockFetch = makeMockFetch({
    '/v1/models': { status: 500, body: { error: 'upstream down' } },
  });

  const ctx = await createTestServer({
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await mutation(ctx.base, 'PUT', '/settings', {
      url: 'https://proxy.example.com',
      apiKey: 'test-key',
      managementKey: 'test-mgmt',
    }, freshKey());
    assert.equal(res.status, 502);
    assert.equal(ctx.currentSettings, null, 'Settings must not be saved on failed upstream test');
  } finally {
    await ctx.cleanup();
  }
});

test('quota refresh returns quotas without exposing provider tokens', async () => {
  const mockFetch = makeMockFetch({
    '/v0/management/auth-files': {
      status: 200,
      body: {
        files: [{
          id: 'acc-claude', name: 'claude-acct', auth_index: 0, provider: 'claude',
          type: 'oauth', label: 'Claude', disabled: false, unavailable: false,
          next_retry_after: null, project_id: null, quota: null, model_quotas: null,
        }],
      },
    },
    '/v0/management/api-call': {
      status: 200,
      body: { status_code: 200, body: { five_hour: { utilization: 30, resets_at: '2026-10-02T06:00:00Z' } } },
    },
    '/v1/models': { status: 200, body: { data: [] } },
    '/v0/management/config': { status: 200, body: {} },
  });

  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/refresh', { id: 'acc-claude' }, freshKey());
    assert.equal(res.status, 200);
    const snapshot = await res.json() as CliRouterSnapshot;
    const account = snapshot.accounts.find(a => a.id === 'acc-claude');
    assert.ok(account);
    assert.ok(account.quotas.length > 0, 'Should have quota data');
    assert.equal(account.quotas[0].label, 'five_hour');
    assert.equal(account.quotas[0].remainingPercent, 70); // utilization is 0..100 percent

    // Verify no tokens/keys leaked
    const text = JSON.stringify(snapshot);
    assert.ok(!text.includes('Bearer'));
    assert.ok(!text.includes('Authorization'));
  } finally {
    await ctx.cleanup();
  }
});

test('unsupported provider quota returns explicit error, not zero', async () => {
  const mockFetch = makeMockFetch({
    '/v0/management/auth-files': {
      status: 200,
      body: {
        files: [{
          id: 'acc-unknown', name: 'custom', auth_index: 0, provider: 'custom-provider',
          type: 'key', label: 'Custom', disabled: false, unavailable: false,
          next_retry_after: null, project_id: null, quota: null, model_quotas: null,
        }],
      },
    },
    '/v1/models': { status: 200, body: { data: [] } },
    '/v0/management/config': { status: 200, body: {} },
  });

  const ctx = await createTestServer({
    settings: { url: 'https://proxy.example.com', apiKey: 'k', managementKey: 'k' },
    mockFetch: mockFetch as typeof globalThis.fetch,
  });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/refresh', { id: 'acc-unknown' }, freshKey());
    assert.equal(res.status, 200);
    const snapshot = await res.json() as CliRouterSnapshot;
    const account = snapshot.accounts.find(a => a.id === 'acc-unknown');
    assert.ok(account);
    assert.ok(account.quotaError, 'Unsupported provider must have explicit quotaError');
    assert.ok(account.quotaError!.includes('not supported'));
    assert.ok(!account.quotaError!.includes('custom-provider'), 'must not echo upstream strings');
    // Must not return zero quotas that could be confused with real data
    assert.deepStrictEqual(account.quotas, []);
  } finally {
    await ctx.cleanup();
  }
});

// ---------------------------------------------------------------------------
// Protocol / validation regressions
// ---------------------------------------------------------------------------

interface Call { url: string; method: string; body: any }

/** Records every upstream call; `handler` returns {status, body} per URL/method. */
function recordingFetch(handler: (c: Call) => { status: number; body: unknown } | undefined) {
  const calls: Call[] = [];
  const fn = async (url: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const c: Call = { url: String(url), method: init?.method ?? 'GET', body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined };
    calls.push(c);
    const r = handler(c) ?? { status: 404, body: {} };
    return new Response(JSON.stringify(r.body), { status: r.status });
  };
  return { calls, fn: fn as typeof globalThis.fetch };
}

const file = (over: Record<string, unknown>) => ({
  id: 'a1', name: 'a1.json', auth_index: 'idx1', provider: 'claude', type: 'claude', label: 'L',
  disabled: false, unavailable: false, next_retry_after: null, project_id: null, ...over,
});

async function refreshOne(f: Record<string, unknown>, providerBody: unknown, providerStatus = 200) {
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/v0/management/auth-files')) return { status: 200, body: { files: [f] } };
    if (c.url.endsWith('/v0/management/api-call')) return { status: 200, body: { status_code: providerStatus, body: providerBody } };
    if (c.url.endsWith('/v1/models')) return { status: 200, body: { data: [] } };
    if (c.url.endsWith('/v0/management/config')) return { status: 200, body: { routing: { strategy: 'round-robin' } } };
  });
  const ctx = await createTestServer({ settings: { url: 'https://r.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/refresh', { id: f.id as string }, freshKey());
    const snap = await res.json() as CliRouterSnapshot;
    return { snap, account: snap.accounts[0], call: rec.calls.find(c => c.url.endsWith('/api-call'))!, status: res.status };
  } finally { await ctx.cleanup(); }
}

test('api-call payload uses header/data, $TOKEN$ auth and server-chosen targets for every provider', async () => {
  const ag = await refreshOne(file({ provider: 'antigravity', project_id: 'proj-1' }),
    JSON.stringify({ groups: [{ displayName: 'Claude models', buckets: [{ displayName: 'Claude', window: '5h', remainingFraction: 0.25, resetTime: '2026-10-02T06:00:00Z' }] }] }));
  assert.equal(ag.call.body.auth_index, 'idx1');
  assert.equal(ag.call.body.header.Authorization, 'Bearer $TOKEN$');
  assert.equal(ag.call.body.header['Content-Type'], 'application/json');
  assert.match(ag.call.body.header['User-Agent'], /^antigravity\/cli\//);
  assert.equal(ag.call.body.url, 'https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary');
  assert.deepEqual(JSON.parse(ag.call.body.data), { project: 'proj-1' });
  assert.equal(ag.call.body.headers, undefined);
  assert.equal(ag.call.body.body, undefined);
  assert.deepEqual(ag.account.quotas.map(q => [q.label, q.remainingPercent, q.resetAt]), [['Claude models · Claude (5h)', 25, '2026-10-02T06:00:00.000Z']]);

  const cl = await refreshOne(file({}), { five_hour: { utilization: 30, resets_at: '2026-10-02T06:00:00Z' } });
  assert.equal(cl.call.body.header.Authorization, 'Bearer $TOKEN$');
  assert.equal(cl.call.body.header['anthropic-beta'], 'oauth-2025-04-20');
  assert.equal(cl.call.body.data, undefined);

  const cx = await refreshOne(file({ provider: 'codex', id_token: { chatgpt_account_id: 'acct-9', email: 'x@y.z' } }), { rate_limit: { primary_window: { used_percent: 40, reset_at: 1790920800 } } });
  assert.equal(cx.call.body.header.Authorization, 'Bearer $TOKEN$');
  assert.equal(cx.call.body.header['Chatgpt-Account-Id'], 'acct-9');
  assert.deepEqual(cx.account.quotas.map(q => [q.label, q.remainingPercent, q.resetAt]), [['primary_window', 60, new Date(1790920800_000).toISOString()]]);
  assert.ok(!JSON.stringify(cx.snap).includes('x@y.z'));
});

test('quota values outside the valid range are unknown, never clamped into plausible numbers', async () => {
  const r = await refreshOne(file({}), {
    five_hour: { utilization: 150 }, seven_day: { utilization: -5 }, seven_day_sonnet: { utilization: 'x' },
    extra_usage: { is_enabled: false },
  });
  assert.deepEqual(r.account.quotas.map(q => [q.label, q.remainingPercent]), [['five_hour', null], ['seven_day', null], ['seven_day_sonnet', null]]);
});

test('provider failures and malformed JSON surface a fixed error with no upstream text', async () => {
  const secret = 'sk-ant-SECRET-token';
  for (const [body, status] of [[`{"oops": "${secret}"`, 200], [{ error: secret }, 401]] as const) {
    const r = await refreshOne(file({}), body, status);
    assert.equal(r.account.quotaError, 'Failed to refresh quota from provider');
    assert.deepEqual(r.account.quotas, []);
    assert.ok(!JSON.stringify(r.snap).includes(secret));
  }
  const noProject = await refreshOne(file({ provider: 'antigravity', project_id: null }), {});
  assert.equal(noProject.account.quotaError, 'Account has no project id');
});

test('auth-file and model normalization: absent fields, bounded strings, zero-time resets', async () => {
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [
      { id: 'x', name: 'x.json', type: 'codex', next_retry_after: '0001-01-01T00:00:00Z' },
      { id: 'y', name: 'y.json', provider: 'claude', disabled: 'yes', next_retry_after: '2026-10-02T06:00:00Z' },
      { name: 'no-id.json' },
      { id: 'z'.repeat(500), name: 'long.json' },
      null,
    ] } };
    if (c.url.endsWith('/v1/models')) return { status: 200, body: { data: [{ id: 'm', owned_by: 7 }, { id: 5 }, { id: 'k'.repeat(500) }, { id: 'ok', owned_by: 'antigravity' }] } };
    if (c.url.endsWith('/v0/management/config')) return { status: 200, body: { routing: { strategy: 'fill-first', 'session-affinity': 'true' } } };
  });
  const ctx = await createTestServer({ settings: { url: 'https://r.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
  try {
    const snap = await (await get(ctx.base, '/')).json() as CliRouterSnapshot;
    assert.deepEqual(snap.accounts.map(a => [a.id, a.provider, a.label, a.disabled, a.unavailable, a.resetAt]), [
      ['x', 'codex', 'x.json', false, false, null],
      ['y', 'claude', 'y.json', false, false, '2026-10-02T06:00:00.000Z'],
    ]);
    assert.deepEqual(snap.models, [{ id: 'm', ownedBy: 'unknown' }, { id: 'ok', ownedBy: 'antigravity' }]);
    assert.equal(snap.sessionAffinity, false, 'non-boolean affinity is not truthy');
  } finally { await ctx.cleanup(); }
});

test('quota cache is keyed per origin and account identity', async () => {
  const files = [file({})];
  let origin = '';
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files } };
    if (c.url.endsWith('/api-call')) return { status: 200, body: { status_code: 200, body: { five_hour: { utilization: 10 } } } };
    if (c.url.endsWith('/v1/models')) return { status: 200, body: { data: [] } };
    if (c.url.endsWith('/v0/management/config')) return { status: 200, body: { routing: { strategy: 'round-robin' } } };
  });
  let stored: CliRouterStoredSettings = { url: 'https://one.example.com', apiKey: 'k', managementKey: 'm' };
  const directory = mkdtempSync(join(tmpdir(), 'gah-cli-router-cache-'));
  const app = express();
  app.use(express.json());
  app.use(rateLimit({ windowMs: 60_000, limit: 200, validate: false }));
  app.use('/api', authMiddleware);
  app.use('/api/cli-router', cliRouterRouter(mutationSafety('cache', directory), { fetchFn: rec.fn, readSettingsFn: () => stored, writeSettingsFn: () => {}, autoRefresh: false, recordQuotaFn: async () => {} }));
  const server = http.createServer(app);
  await new Promise<void>(r => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  try {
    await mutation(base, 'POST', '/accounts/refresh', { id: 'a1' }, freshKey());
    let snap = await (await get(base, '/')).json() as CliRouterSnapshot;
    assert.equal(snap.accounts[0].quotas.length, 1);
    stored = { ...stored, url: 'https://two.example.com' };
    snap = await (await get(base, '/')).json() as CliRouterSnapshot;
    assert.deepEqual(snap.accounts[0].quotas, [], 'other origin must not see old quotas');
    stored = { ...stored, url: 'https://one.example.com' };
    files[0] = file({ auth_index: 'rotated' }) as typeof files[0];
    snap = await (await get(base, '/')).json() as CliRouterSnapshot;
    assert.deepEqual(snap.accounts[0].quotas, [], 'new credential index must not inherit quotas');
  } finally {
    await new Promise<void>(r => server.close(() => r()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test('failed refresh drops previous quotas instead of showing them as current', async () => {
  let fail = false;
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [file({})] } };
    if (c.url.endsWith('/api-call')) return fail ? { status: 500, body: {} } : { status: 200, body: { status_code: 200, body: { five_hour: { utilization: 10 } } } };
    if (c.url.endsWith('/v1/models')) return { status: 200, body: { data: [] } };
    if (c.url.endsWith('/v0/management/config')) return { status: 200, body: { routing: { strategy: 'round-robin' } } };
  });
  const ctx = await createTestServer({ settings: { url: 'https://stale.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
  try {
    await mutation(ctx.base, 'POST', '/accounts/refresh', { id: 'a1' }, freshKey());
    fail = true;
    const snap = await (await mutation(ctx.base, 'POST', '/accounts/refresh', { id: 'a1' }, freshKey())).json() as CliRouterSnapshot;
    assert.deepEqual(snap.accounts[0].quotas, []);
    assert.ok(snap.accounts[0].quotaError);
  } finally { await ctx.cleanup(); }
});

test('routing: exact upstream paths, status checks, and honest partial failure', async () => {
  const run = async (strategyStatus: number, affinityStatus: number) => {
    const rec = recordingFetch(c => {
      if (c.url.endsWith('/v0/management/routing/strategy')) return { status: strategyStatus, body: {} };
      if (c.url.endsWith('/v8/management/config/routing/session-affinity')) return { status: affinityStatus, body: {} };
    });
    const ctx = await createTestServer({ settings: { url: 'https://r.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
    try {
      const res = await mutation(ctx.base, 'POST', '/routing', { strategy: 'fill-first', sessionAffinity: true }, freshKey());
      return { status: res.status, body: await res.json() as { error?: string; message?: string }, calls: rec.calls };
    } finally { await ctx.cleanup(); }
  };
  const ok = await run(200, 200);
  assert.equal(ok.status, 200);
  assert.deepEqual(ok.calls.map(c => [c.method, new URL(c.url).pathname, c.body]), [
    ['PUT', '/v0/management/routing/strategy', { value: 'fill-first' }],
    ['PATCH', '/v8/management/config/routing/session-affinity', true],
  ]);
  const none = await run(404, 200);
  assert.equal(none.status, 502);
  assert.equal(none.body.error, 'routing_update_failed');
  assert.equal(none.calls.length, 1, 'affinity is not attempted after the strategy write failed');
  const partial = await run(200, 500);
  assert.equal(partial.status, 502);
  assert.equal(partial.body.error, 'routing_update_partial_failure');
  assert.match(partial.body.message!, /may have changed/);
});

test('account status checks the upstream HTTP status before reporting success', async () => {
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [file({})] } };
    if (c.url.endsWith('/auth-files/status')) return { status: 500, body: { error: 'x' } };
  });
  const ctx = await createTestServer({ settings: { url: 'https://r.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
  try {
    const res = await mutation(ctx.base, 'POST', '/accounts/status', { id: 'a1', disabled: true }, freshKey());
    assert.equal(res.status, 502);
    assert.deepEqual(rec.calls.find(c => c.url.endsWith('/status'))!.body, { name: 'a1.json', disabled: true });
  } finally { await ctx.cleanup(); }
});

test('GET reports unavailable (not default strategy as live) when routing config fails or strategy is unknown', async () => {
  for (const config of [{ status: 500, body: {} }, { status: 200, body: { routing: { strategy: 'mystery' } } }]) {
    const rec = recordingFetch(c => {
      if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [file({})] } };
      if (c.url.endsWith('/v1/models')) return { status: 200, body: { data: [] } };
      if (c.url.endsWith('/v0/management/config')) return config;
    });
    const ctx = await createTestServer({ settings: { url: 'https://r.example.com', apiKey: 'k', managementKey: 'm' }, mockFetch: rec.fn });
    try {
      const snap = await (await get(ctx.base, '/')).json() as CliRouterSnapshot;
      assert.equal(snap.status, 'unavailable');
    } finally { await ctx.cleanup(); }
  }
});

test('settings file: corrupt reads throw, atomic private write, unique temp, no leftovers', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-cli-router-fs-'));
  const path = join(dir, 'sub', 'cli-router.json');
  const prev = process.env.GAH_CLI_ROUTER_SETTINGS_PATH;
  process.env.GAH_CLI_ROUTER_SETTINGS_PATH = path;
  try {
    assert.equal(readSettings(), null);
    writeSettings({ url: 'https://r.example.com', apiKey: 'a', managementKey: 'm' });
    assert.equal(statSync(path).mode & 0o777, 0o600);
    assert.deepEqual(readdirSync(join(dir, 'sub')), ['cli-router.json']);
    assert.deepEqual(readSettings(), { url: 'https://r.example.com', apiKey: 'a', managementKey: 'm' });
    for (const bad of ['{not json', '[]', '{"url":"http://example.com","apiKey":"a","managementKey":"m"}', '{"url":"https://r.example.com","apiKey":1,"managementKey":"m"}',
      JSON.stringify({ url: 'https://r.example.com', apiKey: 'a\u0000b', managementKey: 'm' })]) {
      writeFileSync(path, bad);
      assert.throws(() => readSettings(), /corrupted/);
    }
  } finally {
    if (prev === undefined) delete process.env.GAH_CLI_ROUTER_SETTINGS_PATH; else process.env.GAH_CLI_ROUTER_SETTINGS_PATH = prev;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('corrupt stored settings: GET and mutations return 500 without network access or overwrite', async () => {
  const rec = recordingFetch(() => ({ status: 200, body: {} }));
  const directory = mkdtempSync(join(tmpdir(), 'gah-cli-router-corrupt-'));
  const writes: unknown[] = [];
  const app = express();
  app.use(express.json());
  app.use(rateLimit({ windowMs: 60_000, limit: 200, validate: false }));
  app.use('/api', authMiddleware);
  app.use('/api/cli-router', cliRouterRouter(mutationSafety('corrupt', directory), {
    fetchFn: rec.fn, readSettingsFn: () => { throw new Error('boom /secret/path'); }, writeSettingsFn: s => { writes.push(s); },
  }));
  const server = http.createServer(app);
  await new Promise<void>(r => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  try {
    assert.equal((await get(base, '/')).status, 500);
    const put = await mutation(base, 'PUT', '/settings', { url: 'https://r.example.com', apiKey: 'a', managementKey: 'm' }, freshKey());
    assert.equal(put.status, 500);
    assert.ok(!JSON.stringify(await put.json()).includes('/secret/path'));
    assert.equal((await mutation(base, 'POST', '/routing', { strategy: 'fill-first', sessionAffinity: false }, freshKey())).status, 500);
    assert.equal((await mutation(base, 'POST', '/accounts/refresh', { id: 'a1' }, freshKey())).status, 500);
    assert.equal(writes.length, 0);
    assert.equal(rec.calls.length, 0);
  } finally {
    await new Promise<void>(r => server.close(() => r()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test('PUT settings rejects non-string or control-char keys and array bodies', async () => {
  const ctx = await createTestServer();
  try {
    for (const body of [{ url: 'https://r.example.com', apiKey: 5 }, { url: 'https://r.example.com', apiKey: 'a\nb', managementKey: 'm' }, ['x']]) {
      const res = await mutation(ctx.base, 'PUT', '/settings', body as object, freshKey());
      assert.equal(res.status, 400);
    }
  } finally { await ctx.cleanup(); }
});

test('URL validation: localhost DNS name is rejected over HTTP', () => {
  assert.throws(() => validateRouterUrl('http://localhost:8317'));
  assert.doesNotThrow(() => validateRouterUrl('http://[::1]:8317'));
});

test('changing router origin requires both new keys before any connection attempt', async () => {
  const rec = recordingFetch(() => ({ status: 200, body: {} }));
  const ctx = await createTestServer({ settings: { url: 'https://old.example.com', apiKey: 'old-api', managementKey: 'old-management' }, mockFetch: rec.fn });
  try {
    for (const keys of [{}, { apiKey: 'new-api' }, { managementKey: 'new-management' }]) {
      const res = await mutation(ctx.base, 'PUT', '/settings', { url: 'https://new.example.com', ...keys }, freshKey());
      assert.equal(res.status, 400);
      assert.equal((await res.json() as { error: string }).error, 'keys_required');
    }
    assert.equal(rec.calls.length, 0);
    assert.equal(ctx.currentSettings?.url, 'https://old.example.com');
  } finally { await ctx.cleanup(); }
});


test('server lifecycle collects quota without HTTP readers, throttles inventory and stops on shutdown', async t => {
  t.mock.timers.enable({ apis: ['setInterval'] });
  let now = Date.now();
  const records: Record<string, unknown>[] = [];
  const rec = recordingFetch(c => {
    if (c.url.endsWith('/auth-files')) return { status: 200, body: { files: [file({ id: 'background', provider: 'claude' })] } };
    if (c.url.endsWith('/api-call')) return { status: 200, body: { status_code: 200, body: { seven_day: { utilization: 20, resets_at: '2026-10-09T18:00:00Z' } } } };
  });
  const observer = createCliRouterQuotaObserver({
    readSettingsFn: () => ({ url: 'https://background.example.com', apiKey: 'k', managementKey: 'm', accountBackends: { background: 'claude' } }),
    fetchFn: rec.fn, now: () => now, recordQuotaFn: async record => { records.push(record); }
  });
  const stop = observer.start();
  try {
    await observer.refresh();
    assert.equal(records.length, 1, 'no dashboard or HTTP request was made');
    assert.equal(records[0].quota_window, 'seven_day');
    await Promise.all([observer.refresh(), observer.refresh()]);
    assert.equal(rec.calls.length, 2, 'inventory and usage are both throttled');
    now += 15 * 60_000;
    t.mock.timers.tick(15 * 60_000);
    await observer.refresh();
    assert.equal(records.length, 2);
    stop();
    now += 15 * 60_000;
    t.mock.timers.tick(15 * 60_000);
    await observer.refresh();
    assert.equal(records.length, 2, 'shutdown prevents future refresh');
  } finally { stop(); }
});

test('shutdown prevents in-flight quota publication', async () => {
  let started!: () => void;
  const inFlight = new Promise<void>(resolve => { started = resolve; });
  let release!: () => void;
  const waiting = new Promise<void>(resolve => { release = resolve; });
  const records: Record<string, unknown>[] = [];
  const observer = createCliRouterQuotaObserver({
    readSettingsFn: () => ({ url: 'https://shutdown.example.com', apiKey: 'k', managementKey: 'm' }),
    fetchFn: async (url) => {
      if (String(url).endsWith('/api-call')) { started(); await waiting; }
      return new Response(JSON.stringify(String(url).endsWith('/auth-files')
        ? { files: [file({ provider: 'claude' })] }
        : { status_code: 200, body: { seven_day: { utilization: 20 } } }));
    },
    recordQuotaFn: async record => { records.push(record); }
  });
  const stop = observer.start();
  const refresh = observer.refresh();
  await inFlight;
  stop();
  release();
  await refresh;
  assert.deepEqual(records, []);
});
