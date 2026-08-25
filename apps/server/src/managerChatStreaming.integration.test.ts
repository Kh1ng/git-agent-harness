import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
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
import { writeGatewaySettings } from './gatewaySettingsStore.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

function nextMessage<T extends ServerMessage['type']>(
  ws: WebSocket,
  type: T,
  predicate: (message: Extract<ServerMessage, { type: T }>) => boolean = () => true
): Promise<Extract<ServerMessage, { type: T }>> {
  return new Promise((resolveMessage, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${type}`)), 10_000);
    const onMessage = (raw: WebSocket.RawData) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type === type && predicate(message as Extract<ServerMessage, { type: T }>)) {
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
    clientVersion: 'integration-test',
    profile,
    capabilities: { supportsTerminal: false, supportsNotifications: false, version: '1' }
  } satisfies ClientMessage));
  return ws;
}

async function closeSocket(ws: WebSocket): Promise<void> {
  if (ws.readyState === WebSocket.CLOSED || ws.readyState === WebSocket.CLOSING) return;
  const closed = new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, 2_000);
    ws.once('close', () => { clearTimeout(timer); resolve(); });
  });
  ws.close();
  await closed;
}

/** Collect every message of the given type that arrives while a send is in flight. */
async function collectUntil<T extends ServerMessage['type']>(
  ws: WebSocket,
  type: T,
  until: (messages: Extract<ServerMessage, { type: T }>[]) => boolean,
  timeoutMs = 10_000
): Promise<Extract<ServerMessage, { type: T }>[]> {
  const collected: Extract<ServerMessage, { type: T }>[] = [];
  return new Promise((resolveCollection, reject) => {
    const timer = setTimeout(() => {
      ws.off('message', onMessage);
      reject(new Error(`timed out collecting ${type}; got ${collected.length}`));
    }, timeoutMs);
    const onMessage = (raw: WebSocket.RawData) => {
      const message = JSON.parse(raw.toString()) as ServerMessage;
      if (message.type === type) collected.push(message as Extract<ServerMessage, { type: T }>);
      if (until(collected)) {
        clearTimeout(timer);
        ws.off('message', onMessage);
        resolveCollection(collected);
      }
    };
    ws.on('message', onMessage);
  });
}

interface ChatHarness {
  wsUrl: string;
  profile: string;
  stateDir: string;
  captureCount(): number;
}

/** Boots a real ws handler with a mock hermes backend + mock memory gateway,
 * isolated to a temp state dir, then cleans everything up. */
async function withChatHarness(
  run: (h: ChatHarness) => Promise<void>,
  gatewayHandler?: (req: http.IncomingMessage, res: http.ServerResponse, body: unknown) => void
): Promise<void> {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-chat-harness-'));
  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_CHAT_STATE_DIR = join(stateDir, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(stateDir, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(stateDir, 'manager-chat.json');

  let captures = 0;
  const defaultGateway = (req: http.IncomingMessage, res: http.ServerResponse, body: unknown) => {
    void body;
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: '', memory_count: 0, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      captures += 1;
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  };
  const handler = gatewayHandler ?? defaultGateway;
  const gateway = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (chunk) => { raw += chunk; });
    req.on('end', () => {
      let body: unknown;
      try { body = raw ? JSON.parse(raw) : {}; } catch { body = raw; }
      handler(req, res, body);
    });
  });
  await new Promise<void>((done) => gateway.listen(0, '127.0.0.1', done));
  process.env.TDAI_GATEWAY_URL = `http://127.0.0.1:${(gateway.address() as AddressInfo).port}`;

  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));

  try {
    await run({
      wsUrl: `ws://127.0.0.1:${(server.address() as AddressInfo).port}`,
      profile: `e2e-${Date.now()}`,
      stateDir,
      captureCount: () => captures
    });
  } finally {
    wss.close();
    // Guard against a test leaving a socket open: server.close() waits for
    // connections, so bound it -- a hung teardown must never mask a real
    // assertion failure as a timeout.
    await new Promise<void>((done) => {
      const timer = setTimeout(done, 2_000);
      timer.unref();
      server.close(() => { clearTimeout(timer); done(); });
    });
    await new Promise<void>((done) => gateway.close(() => done()));
    rmSync(stateDir, { recursive: true, force: true });
    process.env = savedEnv;
  }
}
test('assistant chunks stream to every client subscribed to the profile, in order', { timeout: 30_000 }, async () => {
  await withChatHarness(async ({ wsUrl, profile }) => {
    const sender = await connect(wsUrl, profile);
    const observer = await connect(wsUrl, profile);

    // Both sockets subscribe to the same profile; each should receive the
    // same chunks in the same order as the backend emits them.
    const senderChunks = collectUntil(
      sender,
      'manager.chat.chunk',
      (chunks) => chunks.length >= 2,
    );
    const observerChunks = collectUntil(
      observer,
      'manager.chat.chunk',
      (chunks) => chunks.length >= 2,
    );
    const reply = nextMessage(sender, 'manager.chat.reply');

    const codeWord = 'STREAM-CODE-8481';
    sender.send(JSON.stringify({
      type: 'manager.chat.send',
      requestId: 'stream-req-1',
      profile,
      message: `Remember code word ${codeWord}. Reply only OK.`
    } satisfies ClientMessage));

    const [chunksA, chunksB, done] = await Promise.all([senderChunks, observerChunks, reply]);

    // The assembled stream equals the final reply, and chunks were delivered
    // progressively (more than one) to both sockets.
    const assembled = chunksA.map((c) => c.text).join('');
    assert.equal(done.reply, 'OK');
    assert.equal(assembled, done.reply);
    assert.ok(chunksA.length >= 2, 'sender should receive progressive chunks');
    assert.equal(chunksB.length, chunksA.length, 'observer receives the same chunk count');
    assert.deepEqual(
      chunksB.map((c) => c.text),
      chunksA.map((c) => c.text),
      'observer gets the same chunk sequence as the sender'
    );
    for (let index = 1; index < chunksA.length; index++) {
      assert.ok(
        chunksA[index].seq > chunksA[index - 1].seq,
        'seq must be strictly monotonic within a turn'
      );
    }
    // The live tee is scoped to the profile's subscribed clients.
    assert.ok(chunksA.every((c) => c.profile === profile));
    assert.ok(chunksA.every((c) => c.turn === 1));

    await closeSocket(sender);
    await closeSocket(observer);
  });
});

test('cancelling a mid-turn reply closes the turn as cancelled and keeps the partial text', { timeout: 30_000 }, async () => {
  await withChatHarness(async ({ wsUrl, profile, stateDir, captureCount }) => {
    const ws = await connect(wsUrl, profile);

    // Send a slow turn that parks mid-generation after one chunk.
    const firstChunk = nextMessage(ws, 'manager.chat.chunk', (c) => c.text === 'Partial answer ');
    ws.send(JSON.stringify({
      type: 'manager.chat.send',
      requestId: 'slow-req',
      profile,
      message: 'SLOW-REPLY please'
    } satisfies ClientMessage));
    await firstChunk;

    // Cancel mid-turn.
    ws.send(JSON.stringify({
      type: 'manager.chat.cancel',
      requestId: 'cancel-req',
      profile
    } satisfies ClientMessage));

    // The turn ends with the partial text preserved and a [cancelled] marker;
    // a later history refetch reconciles (no manager.chat.reply for it).
    const updated = nextMessage(ws, 'manager.chat.updated', (m) => m.profile === profile);
    await updated;
    const historyRequestId = `hist-${Date.now()}`;
    const history = nextMessage(ws, 'manager.chat.history', (m) => m.requestId === historyRequestId);
    ws.send(JSON.stringify({
      type: 'manager.chat.historyRequest',
      requestId: historyRequestId,
      profile
    } satisfies ClientMessage));
    const view = await history;

    assert.equal(view.streaming, null, 'no turn should be left streaming');
    const texts = view.turns.map((t) => t.text);
    assert.ok(texts.includes('Partial answer '), 'partial reply survives in the transcript');
    assert.ok(texts.includes('[cancelled]'), 'cancelled turn renders as cancelled');

    // The log is closed cleanly: turn/end kind 'cancelled', no crash-repair.
    const events = readLog(profile, { stateDir: join(stateDir, 'chat') });
    const ends = events.filter((e) => e.type === 'turn/end');
    assert.equal(ends.length, 1, 'exactly one turn/end');
    assert.equal(ends[0].reason.kind, 'cancelled');

    // capture() was never called for the cancelled turn.
    assert.equal(captureCount(), 0);

    // A new message dispatched right after the cancel starts a fresh turn
    // (queue released), and its exchange IS captured.
    const reply = nextMessage(ws, 'manager.chat.reply');
    ws.send(JSON.stringify({
      type: 'manager.chat.send',
      requestId: 'after-req',
      profile,
      message: 'Remember code word NEXT-CANCEL-1122. Reply only OK.'
    } satisfies ClientMessage));
    assert.equal((await reply).reply, 'OK');
    assert.equal(captureCount(), 1);

    // Cancelling a turn that already completed is a no-op: no extra turn/end.
    ws.send(JSON.stringify({
      type: 'manager.chat.cancel',
      requestId: 'spurious-cancel',
      profile
    } satisfies ClientMessage));
    await new Promise((done) => setTimeout(done, 300));
    assert.equal(
      readLog(profile, { stateDir: join(stateDir, 'chat') }).filter((e) => e.type === 'turn/end').length,
      2,
      'no spurious turn/end from cancelling a completed turn'
    );

    await closeSocket(ws);
  });
});

test('a configured context budget truncates injected recall and records its provenance', { timeout: 30_000 }, async () => {
  const bigContext = 'R'.repeat(10_000);
  await withChatHarness(async ({ wsUrl, profile, stateDir }) => {
    // The budget is a per-profile policy in the gateway settings store --
    // read live per turn, no restart.
    writeGatewaySettings({ contextPolicies: { [profile]: { budgetChars: 1000, tiers: ['L0', 'L1'] } } });

    const ws = await connect(wsUrl, profile);
    try {
      const reply = nextMessage(ws, 'manager.chat.reply');
      ws.send(JSON.stringify({
        type: 'manager.chat.send',
        requestId: 'budget-req',
        profile,
        message: 'Hello there'
      } satisfies ClientMessage));
      // The mock backend replies deterministically; the turn must complete.
      assert.equal(typeof (await reply).reply, 'string');
      assert.ok((await reply).reply.length > 0);

      // The injected context event records the policy + truncation that
      // produced it (AC7: replay explains itself). `text` is the assembled
      // prompt; the recalled blob (the R's) must never exceed the budget.
      const events = readLog(profile, { stateDir: join(stateDir, 'chat') });
      const inject = events.find((e) => e.type === 'user/message' && e.source === 'inject') as
        | (import('@git-agent-harness/contracts').ChatUserMessage & { source: 'inject' })
        | undefined;
      assert.ok(inject, 'an inject event was logged');
      const recalledPortion = inject.text.match(/R{10,}/)?.[0] ?? '';
      assert.equal(recalledPortion.length, 1000, 'context never exceeds the budget');
      assert.equal(inject.truncated, true);
      assert.deepEqual(inject.policy, { budgetChars: 1000, tiers: ['L0', 'L1'] });
      // Truncation is never silent: the agent is told more exists.
      assert.ok(inject.text.includes('truncated to the context budget'), 'truncation is surfaced, not silent');
    } finally {
      await closeSocket(ws);
    }
  }, (req, res) => {
    if (req.url === '/recall') {
      res.end(JSON.stringify({ context: bigContext, memory_count: 1, code: 0, message: 'ok' }));
    } else if (req.url === '/capture') {
      res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false }));
    } else if (req.url === '/session/end') {
      res.end(JSON.stringify({ flushed: true }));
    } else {
      res.writeHead(404).end();
    }
  });
});
