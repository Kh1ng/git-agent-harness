// Issue #880 follow-up: GET /api/settings/gateway lets an operator copy
// this node's memory gateway URL + API key out of the dashboard. Real
// fake HTTP calls against a real createServer() instance.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';

import { createServer } from './server.js';

async function withServer(testFn: (url: string) => Promise<void>) {
  const app = createServer({});
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
      const body = (await res.json()) as { url: string; apiKeyConfigured: boolean; apiKey: string | null };
      assert.equal(body.url, 'http://127.0.0.1:8420');
      assert.equal(body.apiKeyConfigured, false);
      assert.equal(body.apiKey, null);
    });
  } finally {
    if (savedUrl !== undefined) process.env.TDAI_GATEWAY_URL = savedUrl;
    else delete process.env.TDAI_GATEWAY_URL;
    if (savedKey !== undefined) process.env.TDAI_GATEWAY_API_KEY = savedKey;
  }
});

test('reveals the actual key value when one is configured', async () => {
  const savedKey = process.env.TDAI_GATEWAY_API_KEY;
  process.env.TDAI_GATEWAY_API_KEY = 'test-secret-key';
  try {
    await withServer(async (url) => {
      const res = await fetch(`${url}/api/settings/gateway`);
      const body = (await res.json()) as { apiKeyConfigured: boolean; apiKey: string | null };
      assert.equal(body.apiKeyConfigured, true);
      assert.equal(body.apiKey, 'test-secret-key');
    });
  } finally {
    if (savedKey !== undefined) process.env.TDAI_GATEWAY_API_KEY = savedKey;
    else delete process.env.TDAI_GATEWAY_API_KEY;
  }
});
