// Gateway Settings HTTP behavior through a real createServer() instance,
// including the credential-free summary and explicit bootstrap reveal.
// Also covers PUT and gatewaySettingsStore unit behavior.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { rmSync, existsSync } from 'node:fs';
import type { AddressInfo } from 'node:net';

import { createServer } from './server.js';
import { readGatewaySettings, writeGatewaySettings, effectiveGatewayUrl, gatewayEnabledForProfile, effectiveContextPolicy, applyContextBudget } from './gatewaySettingsStore.js';

async function withServer(
  testFn: (url: string) => Promise<void>,
  options: Parameters<typeof createServer>[0] = {}
) {
  const app = createServer(options);
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, resolve));
  const { port } = server.address() as AddressInfo;
  try {
    await testFn(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

test('reports the configured gateway URL and whether an API key is set, without requiring one to be set', async () => {
  const savedUrl = process.env.TDAI_GATEWAY_URL;
  const savedKey = process.env.TDAI_GATEWAY_API_KEY;
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:8420';
  delete process.env.TDAI_GATEWAY_API_KEY;
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway`);
      assert.equal(res.status, 200);
      const body = (await res.json()) as { url: string; apiKeyConfigured: boolean; apiKey?: string | null };
      assert.equal(body.url, 'http://127.0.0.1:8420');
      assert.equal(body.apiKeyConfigured, false);
      assert.equal(Object.hasOwn(body, 'apiKey'), false);
    });
  } finally {
    if (savedUrl !== undefined) process.env.TDAI_GATEWAY_URL = savedUrl;
    else delete process.env.TDAI_GATEWAY_URL;
    if (savedKey !== undefined) process.env.TDAI_GATEWAY_API_KEY = savedKey;
  }
});

test('GET /api/settings/gateway omits a configured API key from serialized output', async () => {
  const canary = 'GAH_TEST_CANARY_1014_GATEWAY_GET';
  const savedKey = process.env.TDAI_GATEWAY_API_KEY;
  process.env.TDAI_GATEWAY_API_KEY = canary;
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway`);
      const serialized = await res.text();
      const body = JSON.parse(serialized) as { apiKeyConfigured: boolean; apiKey?: string | null };
      assert.equal(res.status, 200);
      assert.equal(body.apiKeyConfigured, true);
      assert.equal(serialized.includes(canary), false);
      assert.equal(Object.hasOwn(body, 'apiKey'), false);
    });
  } finally {
    if (savedKey !== undefined) process.env.TDAI_GATEWAY_API_KEY = savedKey;
    else delete process.env.TDAI_GATEWAY_API_KEY;
  }
});

test('POST /api/settings/gateway/bootstrap-command explicitly returns the remote setup command', async () => {
  const canary = 'GAH_TEST_CANARY_1014_GATEWAY_REVEAL';
  const savedUrl = process.env.TDAI_GATEWAY_URL;
  const savedKey = process.env.TDAI_GATEWAY_API_KEY;
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:8420';
  process.env.TDAI_GATEWAY_API_KEY = canary;
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway/bootstrap-command`, { method: 'POST' });
      const serialized = await res.text();
      const body = JSON.parse(serialized) as { command?: string };
      assert.equal(res.status, 200);
      assert.equal(res.headers.get('cache-control'), 'no-store');
      assert.equal(typeof body.command, 'string');
      assert.equal(serialized.includes(canary), true);
      assert.equal(body.command?.includes('GAH_GATEWAY_MODE=remote'), true);
      assert.equal(body.command?.includes('100.64.0.42:8420'), true);
    }, { detectTailscaleIPv4: async () => '100.64.0.42' });
  } finally {
    if (savedUrl !== undefined) process.env.TDAI_GATEWAY_URL = savedUrl;
    else delete process.env.TDAI_GATEWAY_URL;
    if (savedKey !== undefined) process.env.TDAI_GATEWAY_API_KEY = savedKey;
    else delete process.env.TDAI_GATEWAY_API_KEY;
  }
});

// ── gatewaySettingsStore unit tests ──────────────────────────────────────────

test('gatewaySettingsStore: reads defaults when file missing', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    const s = readGatewaySettings();
    assert.equal(s.url, null);
    assert.equal(s.apiKey, null);
    assert.equal(s.enabled, true);
    assert.deepEqual(s.disabledProfiles, []);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
  }
});

test('gatewaySettingsStore: write then read round-trips', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    writeGatewaySettings({ url: 'http://192.168.5.15:8420', apiKey: 'key1', enabled: false, disabledProfiles: ['qa'] });
    const s = readGatewaySettings();
    assert.equal(s.url, 'http://192.168.5.15:8420');
    assert.equal(s.apiKey, 'key1');
    assert.equal(s.enabled, false);
    assert.deepEqual(s.disabledProfiles, ['qa']);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: partial write merges with existing', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    writeGatewaySettings({ url: 'http://original:8420', apiKey: 'k', enabled: true, disabledProfiles: [] });
    writeGatewaySettings({ enabled: false }); // partial update
    const s = readGatewaySettings();
    assert.equal(s.url, 'http://original:8420'); // preserved
    assert.equal(s.enabled, false);              // updated
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: effectiveGatewayUrl falls back to env var', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  const savedEnv = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = 'http://env-host:8420';
  try {
    const url = effectiveGatewayUrl();
    assert.equal(url, 'http://env-host:8420');
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (savedEnv !== undefined) process.env.TDAI_GATEWAY_URL = savedEnv;
    else delete process.env.TDAI_GATEWAY_URL;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: stored url wins over env var', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  const savedEnv = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = 'http://env-host:8420';
  try {
    writeGatewaySettings({ url: 'http://stored-host:8420', apiKey: null, enabled: true, disabledProfiles: [] });
    const url = effectiveGatewayUrl();
    assert.equal(url, 'http://stored-host:8420');
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (savedEnv !== undefined) process.env.TDAI_GATEWAY_URL = savedEnv;
    else delete process.env.TDAI_GATEWAY_URL;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: per-profile opt-out respects disabledProfiles', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    writeGatewaySettings({ url: 'http://configured-host:8420', apiKey: null, enabled: true, disabledProfiles: ['staging'] });
    assert.equal(gatewayEnabledForProfile('prod'), true);
    assert.equal(gatewayEnabledForProfile('staging'), false);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

// ── #961 memory context policy ─────────────────────────────────────────────

test('gatewaySettingsStore: context policy defaults are empty (unaffected profiles)', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    const s = readGatewaySettings();
    assert.deepEqual(s.contextPolicy, {});
    assert.deepEqual(s.contextPolicies, {});
    const policy = effectiveContextPolicy('prod');
    assert.equal(policy.budgetChars, undefined);
    assert.equal(policy.tiers, undefined);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: per-profile policy merges over the global default field-by-field', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    writeGatewaySettings({
      url: null, apiKey: null, enabled: true, disabledProfiles: [],
      contextPolicy: { budgetChars: 2000, tiers: ['L0', 'L1'] },
      contextPolicies: { 'qa': { budgetChars: 500 } }
    });
    assert.deepEqual(effectiveContextPolicy('prod'), { budgetChars: 2000, tiers: ['L0', 'L1'] });
    assert.deepEqual(effectiveContextPolicy('qa'), { budgetChars: 500, tiers: ['L0', 'L1'] });
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

test('gatewaySettingsStore: a change to policy is visible on the next read (live reload)', () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    assert.equal(effectiveContextPolicy('gah').budgetChars, undefined);
    writeGatewaySettings({ url: null, apiKey: null, enabled: true, disabledProfiles: [], contextPolicy: { budgetChars: 100 } });
    assert.equal(effectiveContextPolicy('gah').budgetChars, 100);
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});

test('applyContextBudget truncates deterministically to the highest-relevance head, never silently', () => {
  const long = 'x'.repeat(10_000);
  const result = applyContextBudget(long, { budgetChars: 1000 });
  assert.equal(result.truncated, true);
  assert.equal(result.text.length, 1000);
  assert.ok(result.text.startsWith('xxx'), 'keeps the head (relevance-ordered)');
  // Within budget: untouched.
  assert.deepEqual(applyContextBudget('short', { budgetChars: 1000 }), { text: 'short', truncated: false });
  // No budget: completely unaffected.
  assert.deepEqual(applyContextBudget(long, {}), { text: long, truncated: false });
});

// ── PUT /api/settings/gateway ─────────────────────────────────────────────

test('PUT /api/settings/gateway persists url and returns updated settings', async () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  const savedUrl = process.env.TDAI_GATEWAY_URL;
  process.env.TDAI_GATEWAY_URL = 'http://127.0.0.1:8420';
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url: 'http://192.168.5.15:8420', apiKey: null, enabled: true })
      });
      assert.equal(res.status, 200);
      const body = (await res.json()) as { url: string; enabled: boolean };
      assert.equal(body.enabled, true);
      // Persisted to file
      const stored = readGatewaySettings();
      assert.equal(stored.url, 'http://192.168.5.15:8420');
    });
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (savedUrl !== undefined) process.env.TDAI_GATEWAY_URL = savedUrl;
    else delete process.env.TDAI_GATEWAY_URL;
    if (existsSync(path)) rmSync(path);
  }
});

test('PUT /api/settings/gateway disabling sets enabled=false', async () => {
  const path = join(tmpdir(), `gah-test-settings-${Date.now()}.json`);
  process.env.GAH_GATEWAY_SETTINGS_PATH = path;
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: false })
      });
      assert.equal(res.status, 200);
      const stored = readGatewaySettings();
      assert.equal(stored.enabled, false);
    });
  } finally {
    delete process.env.GAH_GATEWAY_SETTINGS_PATH;
    if (existsSync(path)) rmSync(path);
  }
});
