// Route-level coverage for GET /api/usage/rollup (#940): the endpoint reads
// GAH's own manager-chat session logs, so the test points GAH_CHAT_STATE_DIR
// at a fabricated state dir and asserts the aggregated payload.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { setChatSessionStoreOptions } from './managerChat/chatSessions.js';
import { appendEvents } from './managerChat/sessionLog.js';

const NOW = Date.parse('2026-08-30T12:00:00Z');

test('GET /api/usage/rollup aggregates session-log usage and bounds the window', async () => {
  resetCachedCoordinatorIdentity();
  const tmpIdentityDir = mkdtempSync(join(tmpdir(), 'gah-test-identity-'));
  const savedIdentityPath = process.env.GAH_COORDINATOR_IDENTITY_PATH;
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(tmpIdentityDir, 'coordinator-identity.json');
  resetCachedCoordinatorIdentity();

  const stateDir = mkdtempSync(join(tmpdir(), 'gah-usage-route-'));
  const savedChatState = process.env.GAH_CHAT_STATE_DIR;
  process.env.GAH_CHAT_STATE_DIR = stateDir;
  setChatSessionStoreOptions({ stateDir });

  const logDir = join(stateDir, 'project-gah', 'session-live1');
  mkdirSync(logDir, { recursive: true });
  writeFileSync(join(logDir, 'session.jsonl'), '');
  appendEvents('gah', [
    { type: 'assistant/message', seq: 1, turn: 1, text: 'burn', backend: 'codex', model: 'gpt-5.3', usage: { input_tokens: 1000, output_tokens: 500, total_tokens: 1500, estimated_cost_usd: 0.05, duration_seconds: 2 }, timestamp: NOW },
    { type: 'assistant/message', seq: 2, turn: 2, text: 'silent', backend: 'agy', model: null, usage: null, timestamp: NOW }
  ], { stateDir, sessionId: 'live1' });

  const app = createServer({});
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, resolve));
  const { port } = (server.address() as AddressInfo);

  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/usage/rollup?profile=gah&days=7`);
    assert.equal(response.status, 200);
    const body = await response.json() as {
      profile: string;
      rows: { backend: string; model: string | null; day: string; turns: number; total_tokens: number }[];
      unattributed_turns: number;
    };
    assert.equal(body.profile, 'gah');
    assert.equal(body.rows.length, 1);
    assert.equal(body.rows[0].backend, 'codex');
    assert.equal(body.rows[0].turns, 1);
    assert.equal(body.rows[0].total_tokens, 1500);
    assert.equal(body.unattributed_turns, 1);

    // days is clamped: 0 and 500 both resolve to a valid window request.
    const clamped = await fetch(`http://127.0.0.1:${port}/api/usage/rollup?profile=gah&days=0`);
    assert.equal(clamped.status, 200);
    const bad = await fetch(`http://127.0.0.1:${port}/api/usage/rollup?profile=gah&days=notanumber`);
    assert.equal(bad.status, 200, 'non-numeric days falls back to the 7-day default');
  } finally {
    server.close();
    setChatSessionStoreOptions({ stateDir: undefined });
    if (savedChatState === undefined) delete process.env.GAH_CHAT_STATE_DIR;
    else process.env.GAH_CHAT_STATE_DIR = savedChatState;
    if (savedIdentityPath === undefined) delete process.env.GAH_COORDINATOR_IDENTITY_PATH;
    else process.env.GAH_COORDINATOR_IDENTITY_PATH = savedIdentityPath;
    resetCachedCoordinatorIdentity();
    rmSync(stateDir, { recursive: true, force: true });
    rmSync(tmpIdentityDir, { recursive: true, force: true });
  }
});
