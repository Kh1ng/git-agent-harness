import assert from 'node:assert/strict';
import { once } from 'node:events';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { WebSocket, WebSocketServer } from 'ws';
import type { ClientMessage, ServerMessage } from '@git-agent-harness/contracts';
import { createWebSocketHandler } from './wsServer.js';
import { readLog } from './managerChat/sessionLog.js';

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
    clientVersion: 'vibe-integration-test',
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

/** #1041: a Manager Chat Vibe turn leaks its model's raw tool request
 * (`read_file\u{C8F0}{...}`, one U+C8F0 separator between name and JSON
 * args) instead of executing it. GAH must decode that wire shape into the
 * structured tool-call stream, gate the read through the normal permission
 * round-trip, execute it in the session worktree, and let the review
 * continue on the result -- never persisting raw syntax as assistant text. */
test('a vibe session decodes a leaked tool request, reads the worktree file after permission, and the review continues', { timeout: 30_000 }, async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-vibe-e2e-'));

  // A real git checkout whose committed file is the one the leaked request
  // wants to read; the session worktree materializes from this branch.
  const checkout = join(stateDir, 'checkout');
  const worktreeBase = join(stateDir, 'worktrees');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  writeFileSync(join(checkout, 'memoryGatewayClient.ts'), 'VIBE-REVIEW-CONTENT-1041\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  // A fake vibe install: the launcher's shebang resolves to a fake
  // interpreter (the same resolution path the real launcher goes through),
  // which leaks the observed wire shape on the first invocation and emits
  // the review once the continuation prompt carries the file content.
  const vibeBinDir = join(stateDir, 'vibe-bin');
  const fakeInterpreter = join(vibeBinDir, 'fake-python3');
  const worktreeRecord = join(vibeBinDir, 'cwd.txt');
  execFileSync('mkdir', ['-p', vibeBinDir]);
  writeFileSync(fakeInterpreter, `#!/bin/sh
prompt=$(cat)
printf '%s' "$PWD" > "${worktreeRecord}"
case "$prompt" in
  *VIBE-REVIEW-CONTENT-1041*)
    echo 'Review complete: the client boundary holds.'
    ;;
  *)
    printf 'read_file죰{"file_path": "%s/memoryGatewayClient.ts"}' "$PWD"
    ;;
esac
`, { mode: 0o755 });
  writeFileSync(join(vibeBinDir, 'vibe'), `#!${fakeInterpreter}\n`, { mode: 0o755 });

  const profile = 'vibe-e2e';
  const profileListPath = join(stateDir, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Vibe E2E',
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
  process.env.PATH = `${vibeBinDir}:${process.env.PATH}`;
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
      type: 'manager.chat.sessionCreate', requestId: 'create', profile, backend: 'vibe'
    } satisfies ClientMessage);
    const sessionId = created.session.id;
    // getcwd() in the backend child reports the canonical worktree path
    // (macOS /var -> /private/var), which is also what the decoded request
    // and the tool plan carry.
    const realWorktree = realpathSync(created.session.worktreePath!);
    const targetPath = join(realWorktree, 'memoryGatewayClient.ts');

    const toolPushes: Extract<ServerMessage, { type: 'manager.chat.toolCall' }>[] = [];
    client.on('message', (raw) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type === 'manager.chat.toolCall') toolPushes.push(message);
    });
    const permissionPush = new Promise<Extract<ServerMessage, { type: 'manager.chat.permission' }>>((resolvePush) => {
      client.on('message', (raw) => {
        const message = JSON.parse(raw.toString()) as ServerMessage;
        if (message.type === 'manager.chat.permission') resolvePush(message);
      });
    });

    const replyPromise = request(client, 'manager.chat.reply', 'turn', {
      type: 'manager.chat.send', requestId: 'turn', profile, message: 'Review memoryGatewayClient.ts.', sessionId
    } satisfies ClientMessage);
    const permission = await permissionPush;
    assert.equal(permission.title, `Read ${targetPath}`);
    assert.deepEqual(permission.options, [
      { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
      { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
    ]);
    client.send(JSON.stringify({
      type: 'manager.chat.permission.respond',
      requestId: 'perm-answer',
      profile,
      sessionId,
      permissionId: permission.permissionId,
      optionId: 'allow-once'
    } satisfies ClientMessage));

    const reply = await replyPromise;
    // The review continued after the read; the raw wire shape never
    // surfaced as the assistant reply.
    assert.equal(reply.reply, 'Review complete: the client boundary holds.');
    assert.equal(reply.backend, 'vibe');

    assert.deepEqual(toolPushes.map((event) => event.status), ['pending', 'completed']);
    assert.ok(toolPushes.every((event) => event.name === 'read_file'));
    assert.deepEqual(toolPushes[0].locations, [targetPath]);
    assert.match(toolPushes[1].summary ?? '', /VIBE-REVIEW-CONTENT-1041/);

    // The fake interpreter ran in the session worktree, so the decoded
    // absolute path it emitted is the file GAH actually read.
    assert.equal(readFileSync(worktreeRecord, 'utf8'), realWorktree);
    assert.ok(existsSync(targetPath));

    const history = await request(client, 'manager.chat.history', 'history', {
      type: 'manager.chat.historyRequest', requestId: 'history', profile, sessionId
    } satisfies ClientMessage);
    const toolTurn = history.turns.find((turn) => turn.role === 'tool');
    assert.ok(toolTurn?.tool, 'the decoded request is a structured tool card in the transcript');
    assert.equal(toolTurn.tool.name, 'read_file');
    assert.equal(toolTurn.tool.status, 'completed');
    assert.deepEqual(toolTurn.tool.locations, [targetPath]);
    assert.ok(
      history.turns.every((turn) => !turn.text.includes('죰')),
      'raw vibe tool syntax is never persisted or rendered as assistant text'
    );

    // The durable session log carries the structured events and the final
    // reply only -- never the leaked syntax.
    const logPath = join(stateDir, 'chat', `project-${profile}`, `session-${sessionId}`, 'session.jsonl');
    const logText = readFileSync(logPath, 'utf8');
    assert.ok(!logText.includes('죰'), 'the raw separator never reaches the session log');
    const events = readLog(profile, { stateDir: join(stateDir, 'chat'), sessionId });
    const toolResult = events.find((event) => event.type === 'tool/result');
    assert.ok(toolResult && toolResult.type === 'tool/result');
    assert.equal(toolResult.name, 'read_file');
    assert.match(toolResult.text, /VIBE-REVIEW-CONTENT-1041/);
    const assistantMessage = events.find((event) => event.type === 'assistant/message');
    assert.ok(assistantMessage && assistantMessage.type === 'assistant/message');
    assert.equal(assistantMessage.text, 'Review complete: the client boundary holds.');
    assert.ok(events.some((event) => event.type === 'permission/request'));
    const decision = events.find((event) => event.type === 'permission/decision');
    assert.ok(decision && decision.type === 'permission/decision');
    assert.equal(decision.optionId, 'allow-once');

    client.close();
    await once(client, 'close');
  } finally {
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
