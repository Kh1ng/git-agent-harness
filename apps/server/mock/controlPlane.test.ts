import assert from 'node:assert/strict';
import { test } from 'node:test';

import { WebSocket } from 'ws';
import type { ClientMessage, ServerMessage } from '@git-agent-harness/contracts';
import { createMockControlPlane, MOCK_SCENARIOS } from './controlPlane.js';

async function post(baseUrl: string, path: string, body: unknown = {}): Promise<Response> {
  return fetch(`${baseUrl}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
}

function nextMessage(ws: WebSocket, type: ServerMessage['type'], timeoutMs = 3_000): Promise<ServerMessage> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${type}`)), timeoutMs);
    const listener = (raw: WebSocket.RawData) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type !== type) return;
      clearTimeout(timer);
      ws.off('message', listener);
      resolve(message);
    };
    ws.on('message', listener);
  });
}

test('named scenarios are discoverable, switchable, and resettable in memory', async () => {
  const running = await createMockControlPlane().listen(0);
  try {
    const catalog = await fetch(`${running.baseUrl}/api/mock/scenarios`).then((response) => response.json()) as {
      active: string;
      scenarios: { name: string }[];
    };
    assert.equal(catalog.active, 'normal');
    assert.deepEqual(catalog.scenarios.map(({ name }) => name), Object.keys(MOCK_SCENARIOS));

    const selected = await post(running.baseUrl, '/api/mock/scenario', { name: 'archive-success' });
    assert.equal(selected.status, 200);
    assert.equal(running.scenario(), 'archive-success');

    const archived = await post(running.baseUrl, '/api/manager-chat/sessions/archive', {
      profile: 'fixture',
      sessionId: 'mock-session-1'
    });
    assert.equal(archived.status, 200);
    assert.equal((await archived.json() as { archivedAt: number | null }).archivedAt === null, false);

    await post(running.baseUrl, '/api/mock/reset');
    const sessions = await fetch(`${running.baseUrl}/api/manager-chat/sessions?profile=fixture`).then((response) => response.json()) as {
      sessions: { archivedAt: number | null }[];
    };
    assert.equal(sessions.sessions[0]?.archivedAt, null);

    const storage = await fetch(`${running.baseUrl}/api/manager-chat/storage?profile=fixture`).then((response) => response.json()) as {
      dryRun: boolean;
      candidates: { sessionId: string; reclaimBytes: number }[];
    };
    assert.equal(storage.dryRun, true);
    assert.deepEqual(storage.candidates, [{ profile: 'fixture', sessionId: 'mock-session-1', outcome: 'archived', reason: 'idle', reclaimBytes: 12_582_912 }]);

    const bulk = await post(running.baseUrl, '/api/manager-chat/sessions/archive', {
      profile: 'fixture', sessionIds: ['mock-session-1']
    });
    assert.equal(bulk.status, 200);
    assert.equal((await bulk.json() as { sessions: { outcome: string }[] }).sessions[0]?.outcome, 'archived');
  } finally {
    await running.close();
  }
});

test('normal scenario streams contract-shaped chunks, tools, and completion over a real socket', async () => {
  const running = await createMockControlPlane({ scenario: 'normal' }).listen(0);
  const ws = new WebSocket(running.wsUrl);
  try {
    const welcomePromise = nextMessage(ws, 'server.welcome');
    await new Promise<void>((resolve, reject) => {
      ws.once('open', resolve);
      ws.once('error', reject);
    });
    await welcomePromise;

    ws.send(JSON.stringify({
      type: 'manager.chat.historyRequest',
      requestId: 'history-1',
      profile: 'fixture'
    } satisfies ClientMessage));
    const history = await nextMessage(ws, 'manager.chat.history');
    assert.equal(history.type === 'manager.chat.history' && history.turns.length, 0);

    const chunkPromise = nextMessage(ws, 'manager.chat.chunk');
    const toolPromise = nextMessage(ws, 'manager.chat.toolCall');
    const replyPromise = nextMessage(ws, 'manager.chat.reply');
    ws.send(JSON.stringify({
      type: 'manager.chat.send',
      requestId: 'turn-1',
      profile: 'fixture',
      message: 'exercise the typed stream'
    } satisfies ClientMessage));

    const chunk = await chunkPromise;
    const tool = await toolPromise;
    const reply = await replyPromise;
    assert.equal(chunk.type, 'manager.chat.chunk');
    assert.equal(tool.type, 'manager.chat.toolCall');
    assert.equal(reply.type === 'manager.chat.reply' && reply.reply, 'Mock turn complete after multiple chunks.');
  } finally {
    ws.close();
    await running.close();
  }
});

test('failure scenarios are explicit REST and WS failures with no state writes', async () => {
  const running = await createMockControlPlane({ scenario: 'archive-failure' }).listen(0);
  try {
    const failed = await post(running.baseUrl, '/api/manager-chat/sessions/archive', {
      profile: 'fixture', sessionId: 'mock-session-1'
    });
    assert.equal(failed.status, 502);
    const sessions = await fetch(`${running.baseUrl}/api/manager-chat/sessions?profile=fixture`).then((response) => response.json()) as {
      sessions: { archivedAt: number | null }[];
    };
    assert.equal(sessions.sessions[0]?.archivedAt, null);

    await post(running.baseUrl, '/api/mock/scenario', { name: 'ws-error' });
    const ws = new WebSocket(running.wsUrl);
    const welcomePromise = nextMessage(ws, 'server.welcome');
    await new Promise<void>((resolve, reject) => {
      ws.once('open', resolve);
      ws.once('error', reject);
    });
    await welcomePromise;
    const errorPromise = nextMessage(ws, 'error');
    ws.send(JSON.stringify({
      type: 'manager.chat.send', requestId: 'turn-error', profile: 'fixture', message: 'fail'
    } satisfies ClientMessage));
    const error = await errorPromise;
    assert.equal(error.type === 'error' && error.error, 'Mock WebSocket turn failure');
    ws.close();
  } finally {
    await running.close();
  }
});
