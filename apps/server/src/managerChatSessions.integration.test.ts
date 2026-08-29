import assert from 'node:assert/strict';
import { once } from 'node:events';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { WebSocket, WebSocketServer } from 'ws';
import type { ClientMessage, ServerMessage } from '@git-agent-harness/contracts';
import { createWebSocketHandler } from './wsServer.js';
import { previewProxy } from './managerChat/previewProxy.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

function nextMessage<T extends ServerMessage['type']>(
  ws: WebSocket,
  type: T,
  requestId: string
): Promise<Extract<ServerMessage, { type: T }>> {
  return new Promise((resolveMessage, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${type}`)), 10_000);
    const onMessage = (raw: WebSocket.RawData) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type === 'error' && message.requestId === requestId) {
        clearTimeout(timer);
        ws.off('message', onMessage);
        reject(new Error(message.error));
      } else if (message.type === type && 'requestId' in message && message.requestId === requestId) {
        clearTimeout(timer);
        ws.off('message', onMessage);
        resolveMessage(message as Extract<ServerMessage, { type: T }>);
      }
    };
    ws.on('message', onMessage);
  });
}

async function connect(url: string, profile: string): Promise<WebSocket> {
  const ws = new WebSocket(`${url}?profile=${profile}`);
  await once(ws, 'open');
  ws.send(JSON.stringify({
    type: 'client.hello',
    clientVersion: 'session-integration-test',
    profile,
    capabilities: { supportsTerminal: false, supportsNotifications: false, version: '1' }
  } satisfies ClientMessage));
  return ws;
}

function request<T extends ServerMessage['type']>(
  ws: WebSocket,
  type: T,
  requestId: string,
  message: ClientMessage
): Promise<Extract<ServerMessage, { type: T }>> {
  const promise = nextMessage(ws, type, requestId);
  ws.send(JSON.stringify(message));
  return promise;
}

/**
 * WP2 chat sessions: one conversation bound to one worktree. The turn must
 * run with the worktree as its ACP cwd (what makes a worktree
 * interchangeable between models), the log must be session-scoped, and
 * archiving must save uncommitted work as a patch while the branch survives.
 */
test('a chat session runs its turn inside its worktree and archives safely', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-sessions-e2e-'));

  // A real git checkout the fixture gah reports as the profile.
  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'sessions-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Sessions E2E',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  // Minimal memory gateway: recall returns nothing, capture accepts.
  const gateway = http.createServer((req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));

  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const wsUrl = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;

  try {
    const ws = await connect(wsUrl, profile);

    const created = await request(ws, 'manager.chat.sessionCreated', 'create', {
      type: 'manager.chat.sessionCreate', requestId: 'create', profile
    } satisfies ClientMessage);
    const session = created.session;
    assert.ok(session.worktreePath, 'session created with a worktree');
    assert.ok(existsSync(session.worktreePath), 'worktree exists on disk');
    assert.match(session.worktreePath, /gah-chat-repo-/);

    // The turn must run with the session's worktree as its ACP cwd.
    const reply = await request(ws, 'manager.chat.reply', 'turn', {
      type: 'manager.chat.send', requestId: 'turn', profile, message: 'report cwd', sessionId: session.id
    } satisfies ClientMessage);
    assert.equal(reply.reply, session.worktreePath, 'the backend ran in the session worktree');
    assert.equal(reply.sessionId, session.id);

    // The turn landed in the session's log, not the profile default log.
    const sessionLog = join(stateDir, 'chat', `project-${profile}`, `session-${session.id}`, 'session.jsonl');
    assert.ok(existsSync(sessionLog), 'session-scoped log written');
    const logText = readFileSync(sessionLog, 'utf8');
    assert.match(logText, /report cwd/, 'the prompt is in the session log');
    const defaultLog = join(stateDir, 'chat', `project-${profile}`, 'session-default', 'session.jsonl');
    if (existsSync(defaultLog)) {
      assert.doesNotMatch(readFileSync(defaultLog, 'utf8'), /report cwd/, 'default log untouched');
    }

    // History is session-scoped too.
    const history = await request(ws, 'manager.chat.history', 'history', {
      type: 'manager.chat.historyRequest', requestId: 'history', profile, sessionId: session.id
    } satisfies ClientMessage);
    assert.ok(history.turns.some((turn) => turn.text === session.worktreePath), 'history from the session log');

    // Uncommitted work is preserved as a patch on archive; branch survives.
    writeFileSync(join(session.worktreePath!, 'uncommitted.txt'), 'in flight\n');
    const archived = await request(ws, 'manager.chat.sessionArchived', 'archive', {
      type: 'manager.chat.sessionArchive', requestId: 'archive', profile, sessionId: session.id
    } satisfies ClientMessage);
    assert.ok(archived.session.archivedAt !== null);
    assert.ok(!existsSync(session.worktreePath!), 'worktree removed on archive');
    const branches = execFileSync('git', ['branch', '--list', session.branch], { cwd: checkout, encoding: 'utf8' });
    assert.ok(branches.includes(session.branch), 'branch survives archive');
    const sessionStateDir = join(stateDir, 'chat', `project-${profile}`, `session-${session.id}`);
    const patches = readdirSync(sessionStateDir).filter((f) => f.endsWith('.patch'));
    assert.equal(patches.length, 1, 'dirty work archived as exactly one patch');
    assert.match(readFileSync(join(sessionStateDir, patches[0]), 'utf8'), /uncommitted\.txt/);

    // A turn against the archived session fails loudly, never falls back to
    // the default conversation.
    await assert.rejects(
      request(ws, 'manager.chat.reply', 'turn2', {
        type: 'manager.chat.send', requestId: 'turn2', profile, message: 'report cwd', sessionId: session.id
      } satisfies ClientMessage),
      /No active chat session/
    );

    ws.close();
    await once(ws, 'close');
  } finally {
    wss.close();
    await new Promise<void>((done) => server.close(() => done()));
    await new Promise<void>((done) => gateway.close(() => done()));
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});

/** WP2 per-session provider/model: the new-chat flow pins a backend+model on
 * the session; turns apply the model on the session's own connection, and
 * updating the session's backend/model changes what serves the next turn in
 * the SAME worktree -- the manual form of backend interchange. */
test('a session serves turns on its pinned model and switches model/backend in place', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-model-e2e-'));

  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'model-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Model E2E',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  const gateway = http.createServer((req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));

  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const wsUrl = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;

  try {
    const ws = await connect(wsUrl, profile);

    // Create with a pinned model (mock hermes advertises mock-default /
    // mock-strong and echoes the current model on "report model").
    const created = await request(ws, 'manager.chat.sessionCreated', 'create', {
      type: 'manager.chat.sessionCreate', requestId: 'create', profile, backend: 'hermes', model: 'mock-strong'
    } satisfies ClientMessage);
    assert.equal(created.session.model, 'mock-strong');

    const first = await request(ws, 'manager.chat.reply', 'turn1', {
      type: 'manager.chat.send', requestId: 'turn1', profile, message: 'report model', sessionId: created.session.id
    } satisfies ClientMessage);
    assert.equal(first.reply, 'mock-strong', 'the pinned model served the turn');

    // Update the model in place: same session, same worktree, new model.
    const updated = await request(ws, 'manager.chat.sessionUpdated', 'update', {
      type: 'manager.chat.sessionUpdate', requestId: 'update', profile, sessionId: created.session.id, model: 'mock-default'
    } satisfies ClientMessage);
    assert.equal(updated.session.model, 'mock-default');
    assert.equal(updated.session.worktreePath, created.session.worktreePath, 'worktree unchanged by the switch');

    const second = await request(ws, 'manager.chat.reply', 'turn2', {
      type: 'manager.chat.send', requestId: 'turn2', profile, message: 'report model', sessionId: created.session.id
    } satisfies ClientMessage);
    assert.equal(second.reply, 'mock-default', 'the switched model served the next turn in the same worktree');

    ws.close();
    await once(ws, 'close');
  } finally {
    wss.close();
    await new Promise<void>((done) => server.close(() => done()));
    await new Promise<void>((done) => gateway.close(() => done()));
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});

test('a session pins its reasoning effort, serves turns on it, and switches it in place', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-effort-e2e-'));

  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'effort-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Effort E2E',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  const gateway = http.createServer((req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));

  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const wsUrl = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;

  try {
    const ws = await connect(wsUrl, profile);

    // Create with a pinned effort (mock hermes advertises mock-low /
    // mock-high on the thought_level config option and echoes the current
    // effort on "report effort").
    const created = await request(ws, 'manager.chat.sessionCreated', 'create', {
      type: 'manager.chat.sessionCreate', requestId: 'create', profile, backend: 'hermes', reasoningEffort: 'mock-high'
    } satisfies ClientMessage);
    assert.equal(created.session.reasoningEffort, 'mock-high');

    const first = await request(ws, 'manager.chat.reply', 'turn1', {
      type: 'manager.chat.send', requestId: 'turn1', profile, message: 'report effort', sessionId: created.session.id
    } satisfies ClientMessage);
    assert.equal(first.reply, 'mock-high', 'the pinned effort served the turn');

    // A session created without an effort pin uses the backend default.
    const plain = await request(ws, 'manager.chat.sessionCreated', 'create2', {
      type: 'manager.chat.sessionCreate', requestId: 'create2', profile, backend: 'hermes'
    } satisfies ClientMessage);
    assert.equal(plain.session.reasoningEffort, null);
    const plainTurn = await request(ws, 'manager.chat.reply', 'turn2', {
      type: 'manager.chat.send', requestId: 'turn2', profile, message: 'report effort', sessionId: plain.session.id
    } satisfies ClientMessage);
    assert.equal(plainTurn.reply, 'mock-low', 'unpinned session served the backend default effort');

    // Switch in place: same session, next turn on the new effort.
    const updated = await request(ws, 'manager.chat.sessionUpdated', 'update', {
      type: 'manager.chat.sessionUpdate', requestId: 'update', profile, sessionId: created.session.id, reasoningEffort: 'mock-low'
    } satisfies ClientMessage);
    assert.equal(updated.session.reasoningEffort, 'mock-low');
    assert.equal(updated.session.worktreePath, created.session.worktreePath, 'worktree unchanged by the switch');

    const third = await request(ws, 'manager.chat.reply', 'turn3', {
      type: 'manager.chat.send', requestId: 'turn3', profile, message: 'report effort', sessionId: created.session.id
    } satisfies ClientMessage);
    assert.equal(third.reply, 'mock-low', 'the switched effort served the next turn in the same worktree');

    // Clearing the pin returns the session to the backend default.
    const cleared = await request(ws, 'manager.chat.sessionUpdated', 'clear', {
      type: 'manager.chat.sessionUpdate', requestId: 'clear', profile, sessionId: created.session.id, reasoningEffort: null
    } satisfies ClientMessage);
    assert.equal(cleared.session.reasoningEffort, null);

    ws.close();
    await once(ws, 'close');
  } finally {
    wss.close();
    await new Promise<void>((done) => server.close(() => done()));
    await new Promise<void>((done) => gateway.close(() => done()));
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});

test('tool calls stream as structured events and permissions round-trip through the client', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-agent-surface-'));

  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'agent-surface-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Agent Surface E2E',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  const gateway = http.createServer((req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));

  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const wsUrl = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;

  let ws: WebSocket | null = null;
  try {
    ws = await connect(wsUrl, profile);
    const client = ws;
    const created = await request(client, 'manager.chat.sessionCreated', 'create', {
      type: 'manager.chat.sessionCreate', requestId: 'create', profile
    } satisfies ClientMessage);
    const sessionId = created.session.id;

    const toolEvents: string[] = [];
    ws.on('message', (raw) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type === 'manager.chat.toolCall') toolEvents.push(message.status);
    });
    const toolReply = await request(client, 'manager.chat.reply', 'turn-tool', {
      type: 'manager.chat.send', requestId: 'turn-tool', profile, message: 'use a tool', sessionId
    } satisfies ClientMessage);

    const permissionPush = new Promise<Extract<ServerMessage, { type: 'manager.chat.permission' }>>((resolve) => {
      client.on('message', (raw) => {
        const message = JSON.parse(raw.toString()) as ServerMessage;
        if (message.type === 'manager.chat.permission') resolve(message);
      });
    });
    const dangerousReplyPromise = request(client, 'manager.chat.reply', 'turn-perm', {
      type: 'manager.chat.send', requestId: 'turn-perm', profile, message: 'do a dangerous thing', sessionId
    } satisfies ClientMessage);
    const permission = await permissionPush;
    ws.send(JSON.stringify({
      type: 'manager.chat.permission.respond',
      requestId: 'perm-answer',
      profile,
      sessionId,
      permissionId: permission.permissionId,
      optionId: 'allow-once'
    } satisfies ClientMessage));
    const dangerousReply = await dangerousReplyPromise;
    assert.equal(dangerousReply.reply, 'permission allow-once');

    const history = await request(client, 'manager.chat.history', 'history', {
      type: 'manager.chat.historyRequest', requestId: 'history', profile, sessionId
    } satisfies ClientMessage);
    const toolTurn = history.turns.find((turn) => turn.role === 'tool');
    assert.ok(toolTurn?.tool, 'tool card present in the folded transcript');
    assert.equal(toolTurn.tool.title, 'Reading README.md');
    assert.equal(toolTurn.tool.status, 'completed');
    assert.deepEqual(toolTurn.tool.locations, ['/tmp/README.md']);
    assert.match(toolTurn.tool.summary ?? '', /tool output line 1/);
    const historyText = history.turns.map((t) => t.text).join('\n');
    assert.match(historyText, /permission requested/);
    assert.match(historyText, /allow-once/);

    client.close();
    await once(client, 'close');
  } finally {
    // Close the test client first: server.close() waits for open sockets,
    // so an assertion failure above (ws never closed in the try) would
    // otherwise wedge the finally forever and mask the real error.
    try { ws?.close(); } catch { /* already closed */ }
    await new Promise((r) => setTimeout(r, 300));
    wss.close();
    await new Promise<void>((done) => server.close(() => done()));
    await new Promise<void>((done) => gateway.close(() => done()));
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});

/** WP3: a dev server started by the agent in the session worktree is
 * auto-detected from tool output, pushed live, and proxied through a
 * dedicated port (Host rewritten to localhost). */
test('a session preview auto-detects the dev-server port and proxies it', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-preview-e2e-'));

  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'preview-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Preview E2E',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  const gateway = http.createServer((req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));

  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const wsUrl = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;

  // Pin the advertised preview host to loopback: the real code path falls
  // back to this node's own Tailscale IPv4 when present (see
  // previewProxy.ts), but a node whose tailscaled runs in userspace-network
  // mode (no TUN) can report a Tailscale IP that black-holes a self-connect
  // from this same process -- the http.get() below would hang until the
  // whole test's 30s timeout, wedging `gah update`'s web-build prebuild
  // gate. Tailscale detection itself is covered hermetically in
  // tailscaleDetect.test.ts; this test only needs a reachable target.
  previewProxy.configure({ advertiseHost: '127.0.0.1' });

  let createdSessionId: string | null = null;
  // The "dev server" the agent allegedly started: records the Host header.
  const seenHosts: string[] = [];
  const devServer = http.createServer((req, res) => {
    seenHosts.push(String(req.headers.host));
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<h1>dev server</h1>');
  });
  await new Promise<void>((done) => devServer.listen(4599, '127.0.0.1', done));

  try {
    const ws = await connect(wsUrl, profile);
    const created = await request(ws, 'manager.chat.sessionCreated', 'create', {
      type: 'manager.chat.sessionCreate', requestId: 'create', profile
    } satisfies ClientMessage);
    const sessionId = created.session.id;

    const previewPush = new Promise<Extract<ServerMessage, { type: 'manager.chat.preview' }>>((resolve) => {
      ws.on('message', (raw) => {
        const message = JSON.parse(raw.toString()) as ServerMessage;
        if (message.type === 'manager.chat.preview') resolve(message);
      });
    });
    const reply = await request(ws, 'manager.chat.reply', 'turn', {
      type: 'manager.chat.send', requestId: 'turn', profile, message: 'start a dev server', sessionId
    } satisfies ClientMessage);
    assert.equal(reply.reply, 'dev server running');
    createdSessionId = sessionId;

    const preview = await previewPush;
    assert.equal(preview.devPort, 4599, 'port auto-detected from the tool output');
    assert.ok(preview.url.includes(`:${preview.listenPort}`));

    // The preview URL actually serves the dev server through the proxy,
    // with the Host header rewritten to localhost (dev-server
    // allowedHosts protection never sees the node's address).
    const served = await new Promise<{ status: number; body: string }>((resolve, reject) => {
      http.get(preview.url, { agent: false }, (res) => {
        let body = '';
        res.on('data', (chunk) => { body += chunk; });
        res.on('end', () => resolve({ status: res.statusCode ?? 0, body }));
      }).on('error', reject);
    });
    assert.equal(served.status, 200);
    assert.match(served.body, /dev server/);
    assert.deepEqual(seenHosts, ['localhost:4599'], 'Host rewritten to localhost');

    ws.close();
    await once(ws, 'close');
  } finally {
    wss.close();
    await new Promise<void>((done) => server.close(() => done()));
    await new Promise<void>((done) => gateway.close(() => done()));
    await new Promise<void>((done) => devServer.close(() => done()));
    if (createdSessionId) await previewProxy.clear(profile, createdSessionId);
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    rmSync(stateDir, { recursive: true, force: true });
  }
});

// Debug-only: if the process is still alive 5s after tests, dump handles.
