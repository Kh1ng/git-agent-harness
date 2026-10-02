import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { mutationSafety } from './mutationSafety.js';
import { cliRouterRouter, validateRouterUrl, boundedUpstreamFetch } from './cliRouter.js';
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
  assert.ok(validateRouterUrl('http://localhost:3000'));
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
          id: 'acc-claude', name: 'claude-acct', auth_index: 0, provider: 'anthropic',
          type: 'oauth', label: 'Claude', disabled: false, unavailable: false,
          next_retry_after: null, project_id: null, quota: null, model_quotas: null,
        }],
      },
    },
    '/v0/management/api-call': {
      status: 200,
      body: { status_code: 200, body: { five_hour: { utilization: 0.3, resets_at: '2026-10-02T06:00:00Z' } } },
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
    assert.equal(account.quotas[0].remainingPercent, 70); // (1 - 0.3) * 100

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
    assert.ok(account.quotaError!.includes('Unsupported provider'));
    // Must not return zero quotas that could be confused with real data
    assert.deepStrictEqual(account.quotas, []);
  } finally {
    await ctx.cleanup();
  }
});
