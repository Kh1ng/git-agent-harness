import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import ts from 'typescript';
import { WebSocket } from 'ws';
import type { ClientMessage, ProjectImportResult, ServerMessage } from '@git-agent-harness/contracts';
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

function frontendApiRoutes(): { method: string; path: string }[] {
  const clientPath = resolve(dirname(fileURLToPath(import.meta.url)), '../../web/src/api/client.ts');
  const source = ts.createSourceFile(clientPath, readFileSync(clientPath, 'utf8'), ts.ScriptTarget.Latest, true);
  const helperMethods = new Map([
    ['getJson', 'GET'],
    ['postJson', 'POST'],
    ['patchJson', 'PATCH'],
    ['putJson', 'PUT'],
    ['deleteJson', 'DELETE']
  ]);
  const routes: { method: string; path: string }[] = [];

  function routePath(node: ts.Expression | undefined): string | null {
    if (!node) return null;
    if (ts.isStringLiteralLike(node)) return node.text.startsWith('/api/') ? node.text : null;
    if (!ts.isTemplateExpression(node) || !node.head.text.startsWith('/api/')) return null;
    return node.head.text + node.templateSpans.map((span) => `:param${span.literal.text}`).join('');
  }

  function visit(node: ts.Node): void {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      const method = helperMethods.get(node.expression.text);
      const path = routePath(node.arguments[0]);
      if (method && path) routes.push({ method, path: path.split('?')[0] });
    } else if (ts.isNewExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'URL') {
      const path = routePath(node.arguments?.[0]);
      if (path) routes.push({ method: 'POST', path: path.split('?')[0] });
    }
    ts.forEachChild(node, visit);
  }
  visit(source);

  const deadClientMethods = new Set([
    'POST /api/projects',
    'DELETE /api/projects/:param',
    'POST /api/context/recall',
    'GET /api/git/branches'
  ]);
  return routes.filter(({ method, path }) => !deadClientMethods.has(`${method} ${path}`));
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

test('PR start opens a worktree-less session whose history is seeded with the PR', async () => {
  const running = await createMockControlPlane().listen(0);
  try {
    const prs = await fetch(`${running.baseUrl}/api/manager-chat/prs?profile=fixture`).then((response) => response.json()) as {
      prs: { number: number; title: string; author: string; isDraft: boolean; reviewState: string | null }[];
    };
    assert.deepEqual(prs.prs.map((pr) => pr.number), [12, 11]);
    assert.equal(prs.prs[0].author, 'octocat');
    assert.equal(prs.prs[1].isDraft, true);

    const missing = await post(running.baseUrl, '/api/manager-chat/prs/start', { profile: 'fixture' });
    assert.equal(missing.status, 400);

    const started = await post(running.baseUrl, '/api/manager-chat/prs/start', { profile: 'fixture', prNumber: 12 });
    assert.equal(started.status, 201);
    const { session } = await started.json() as { session: { id: string; worktreePath: string | null; branch: string; title: string } };
    assert.equal(session.worktreePath, null, 'read-only: no worktree');
    assert.equal(session.branch, 'feat/pr-chat');
    assert.equal(session.title, '#12 Ship the PR chat mode');

    const ws = new WebSocket(running.wsUrl);
    const welcomePromise = nextMessage(ws, 'server.welcome');
    await new Promise<void>((resolve, reject) => {
      ws.once('open', resolve);
      ws.once('error', reject);
    });
    await welcomePromise;
    ws.send(JSON.stringify({
      type: 'manager.chat.historyRequest',
      requestId: 'pr-history',
      profile: 'fixture',
      sessionId: session.id
    } satisfies ClientMessage));
    const history = await nextMessage(ws, 'manager.chat.history');
    assert.equal(history.type === 'manager.chat.history' && history.turns[0]?.role, 'user');
    assert.equal(history.type === 'manager.chat.history' && history.turns[0]?.text.includes('#12 Ship the PR chat mode'), true);
    ws.close();
  } finally {
    await running.close();
  }
});

test('Settings mutations persist without storing secrets and reset to defaults', async () => {
  const running = await createMockControlPlane().listen(0);
  try {
    const gateway = await fetch(`${running.baseUrl}/api/settings/gateway`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ enabled: false, apiKey: 'must-not-serialize', contextPolicy: { budgetChars: 1_200, tiers: ['L0'] } })
    });
    assert.equal(gateway.status, 200);

    const profile = await fetch(`${running.baseUrl}/api/profiles/fixture`, {
      method: 'PATCH',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ validation_timeout_seconds: 900 })
    });
    assert.equal(profile.status, 200);

    const changed = await fetch(`${running.baseUrl}/api/mock/state`).then((response) => response.json()) as {
      gateway: { enabled: boolean; apiKeyConfigured: boolean; contextPolicy: { budgetChars?: number } };
      profiles: { validation_timeout_seconds: number }[];
    };
    assert.equal(changed.gateway.enabled, false);
    assert.equal(changed.gateway.apiKeyConfigured, true);
    assert.equal(changed.gateway.contextPolicy.budgetChars, 1_200);
    assert.equal(changed.profiles[0]?.validation_timeout_seconds, 900);
    assert.equal(JSON.stringify(changed).includes('must-not-serialize'), false);

    await post(running.baseUrl, '/api/mock/reset');
    const reset = await fetch(`${running.baseUrl}/api/mock/state`).then((response) => response.json()) as typeof changed;
    assert.equal(reset.gateway.enabled, true);
    assert.equal(reset.gateway.contextPolicy.budgetChars, 4_000);
    assert.equal(reset.profiles[0]?.validation_timeout_seconds, 300);
  } finally {
    await running.close();
  }
});

test('new non-chat mutations return success and persist in memory', async () => {
  const running = await createMockControlPlane().listen(0);
  try {
    const added = await post(running.baseUrl, '/api/profiles', {
      name: 'added',
      display_name: 'Added',
      repo_id: 'added',
      provider: 'github',
      repo: 'fixture/added',
      local_path: '/mock/added',
      artifact_root: '/mock/artifacts'
    });
    assert.equal(added.status, 201);
    assert.equal((await fetch(`${running.baseUrl}/api/profiles`).then((response) => response.json()) as { name: string }[]).some(({ name }) => name === 'added'), true);

    const patched = await fetch(`${running.baseUrl}/api/profiles/added`, {
      method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ validation_timeout_seconds: 600 })
    });
    assert.equal(patched.status, 200);
    assert.equal((await fetch(`${running.baseUrl}/api/profiles/added`, { method: 'DELETE' })).status, 200);

    const imported = await post(running.baseUrl, '/api/projects/import', { gitUrl: 'https://github.com/fixture/imported.git' });
    assert.equal(imported.status, 201);
    assert.equal((await imported.json() as ProjectImportResult).project.repo, 'fixture/imported');

    assert.equal((await post(running.baseUrl, '/api/loop/stop', { profile: 'fixture' })).status, 200);
    assert.equal((await post(running.baseUrl, '/api/loop/start', { profile: 'fixture' })).status, 200);
    assert.equal((await post(running.baseUrl, '/api/config', { current_manager: 'claude' })).status, 200);

    const pullRequest = await post(running.baseUrl, '/api/git/pr', { title: 'Mock pull request' });
    assert.equal(pullRequest.status, 200);
    const prs = await fetch(`${running.baseUrl}/api/git/prs`).then((response) => response.json()) as { prs: { title: string }[] };
    assert.equal(prs.prs[0]?.title, 'Mock pull request');

    assert.equal((await post(running.baseUrl, '/api/manager-chat/issues/start', { profile: 'fixture', issueNumber: 1087 })).status, 201);
    assert.equal((await post(running.baseUrl, '/api/admin/update')).status, 202);
  } finally {
    await running.close();
  }
});

test('rest-error rejects every used frontend mutation without changing state', async () => {
  const running = await createMockControlPlane({ scenario: 'rest-error' }).listen(0);
  try {
    for (const { method, path } of frontendApiRoutes().filter(({ method }) => method !== 'GET')) {
      const failed = await fetch(`${running.baseUrl}${path.replaceAll(':param', 'fixture')}`, {
        method,
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ enabled: false })
      });
      assert.equal(failed.status, 503, `${method} ${path}`);
    }
    const state = await fetch(`${running.baseUrl}/api/mock/state`).then((response) => response.json()) as {
      gateway: { enabled: boolean };
    };
    assert.equal(state.gateway.enabled, true);
  } finally {
    await running.close();
  }
});

test('every used frontend API call has a registered mock route', async () => {
  const running = await createMockControlPlane().listen(0);
  try {
    for (const { method, path } of frontendApiRoutes()) {
      const requestPath = path.replaceAll(':param', 'fixture');
      const response = await fetch(`${running.baseUrl}${requestPath}`, {
        method,
        ...(method === 'GET' ? {} : {
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({})
        })
      });
      assert.notEqual(response.status, 404, `${method} ${path}`);
    }
  } finally {
    await running.close();
  }
});
