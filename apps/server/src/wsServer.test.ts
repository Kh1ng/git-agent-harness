import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocket, WebSocketServer } from 'ws';
import { RegistryService } from './registryService.js';
import { COORDINATOR_SCHEMA_DIGEST, getCoordinatorIdentity } from './coordinatorIdentity.js';
import { createWebSocketHandler } from './wsServer.js';
import { createAuthorizedWebSocketServer } from './webSocketAuth.js';
import type { NodeRoleStatus, ServerMessage } from '@git-agent-harness/contracts';

// Hermetic: welcome is pushed only after gahCli.runStatus() spawns a real
// `gah status` child. A cold call of the repo's release binary takes ~21s
// here (statusCache then serves subsequent calls), which blows past the
// per-message welcome deadline and makes the first connection a flaky
// ~20s timeout. Point GAH_BINARY at the instant fixture instead.
process.env.GAH_BINARY = fileURLToPath(new URL('../tests/fixtures/gah/gah', import.meta.url));

/**
 * AC3 ("a restored WebSocket connection re-triggers a fresh REST pull",
 * apps/web's useWsReconnectRefresh/reconnectSeq) assumes every new socket
 * connection to the real server -- not just the first one -- gets a fresh
 * server.welcome push, independent of any prior connection's lifecycle.
 * The web e2e spec (dashboard-freshness.spec.ts) only proves the client
 * reacts correctly to whatever a hand-authored Playwright WS mock decides
 * to send; it never touches this file. This exercises the actual
 * production handler (wss.on('connection', ...) -> sendWelcomeMessage)
 * against real `ws` sockets to ground that assumption in the real
 * server/client boundary.
 */
async function withWsServer(testFn: (wsUrl: string) => Promise<void>, node?: NodeRoleStatus) {
  const server = http.createServer();
  const wss = createAuthorizedWebSocketServer(server, node?.role ?? 'central');
  createWebSocketHandler(wss, { node });

  await new Promise<void>((resolve) => server.listen(0, resolve));
  const { port } = server.address() as AddressInfo;

  try {
    await testFn(`ws://127.0.0.1:${port}`);
  } finally {
    wss.close();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

// sendWelcomeMessage awaits a real `gah status --json` child process
// (gahCli.runStatus) before pushing -- typically single-digit seconds on
// first call, then cached (see gahCli.ts's statusCache) for the rest of
// the run. This margin is for that first, uncached call.
function nextWelcome(ws: WebSocket): Promise<ServerMessage & { type: 'server.welcome' }> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for server.welcome')), 15000);
    ws.on('message', (data) => {
      const message = JSON.parse(data.toString()) as ServerMessage;
      if (message.type === 'server.welcome') {
        clearTimeout(timer);
        resolve(message);
      }
    });
    ws.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

async function connectAndAwaitWelcome(wsUrl: string, token?: string): Promise<WebSocket> {
  const ws = new WebSocket(wsUrl, { headers: token ? { Authorization: `Bearer ${token}` } : {} });
  await new Promise<void>((resolve, reject) => {
    ws.on('open', resolve);
    ws.on('error', reject);
  });
  await nextWelcome(ws);
  return ws;
}

test(
  'a fresh connection after a prior one closed still gets its own server.welcome (real handler, real sockets)',
  { timeout: 20000 },
  async () => {
    await withWsServer(async (wsUrl) => {
      const first = await connectAndAwaitWelcome(wsUrl);
      first.close();
      await new Promise<void>((resolve) => first.on('close', () => resolve()));

      // Simulates the client-side reconnect in WebSocketContext.tsx: a brand
      // new socket opened after the previous one dropped. If the real server
      // only pushed state on the very first connection ever, this would hang
      // and nextWelcome's own guard above would time this out.
      const second = await connectAndAwaitWelcome(wsUrl);
      second.close();
    });
  }
);

test(
  'two independent connections each receive their own independent server.welcome',
  { timeout: 20000 },
  async () => {
    await withWsServer(async (wsUrl) => {
      const a = await connectAndAwaitWelcome(wsUrl);
      const b = await connectAndAwaitWelcome(wsUrl);
      assert.notEqual(a, b);
      a.close();
      b.close();
    });
  }
);

test('server.welcome reconciles persisted leases before reporting running sessions', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-welcome-leases-'));
  const leasesPath = join(dir, 'dispatch-leases.json');
  const identity = getCoordinatorIdentity();
  const timestamp = new Date().toISOString();
  const staleLease = {
    requestId: 'stale-request',
    workKey: 'stale-work',
    profile: 'gah',
    nodeId: identity.node_id,
    nodeUrl: identity.advertised_url,
    session: {
      id: 'stale-session',
      providerKind: 'codex',
      instanceId: 'codex-0',
      status: 'running',
      repo: 'owner/repo',
      mode: 'improve'
    },
    state: 'running',
    createdAt: timestamp,
    updatedAt: timestamp,
    coordinatorNodeId: identity.node_id
  };
  writeFileSync(leasesPath, JSON.stringify({
    leases: [
      staleLease,
      {
        ...staleLease,
        requestId: 'other-request',
        workKey: 'other-work',
        profile: 'other',
        session: { ...staleLease.session, id: 'other-session' }
      }
    ]
  }));

  const previousPath = process.env.GAH_DISPATCH_LEASES_PATH;
  process.env.GAH_DISPATCH_LEASES_PATH = leasesPath;
  try {
    await withWsServer(async (wsUrl) => {
      const ws = new WebSocket(wsUrl);
      try {
        const welcome = await nextWelcome(ws);
        assert.deepEqual(welcome.sessions, []);
      } finally {
        ws.close();
      }
    });
  } finally {
    if (previousPath === undefined) delete process.env.GAH_DISPATCH_LEASES_PATH;
    else process.env.GAH_DISPATCH_LEASES_PATH = previousPath;
    rmSync(dir, { recursive: true });
  }
});


test('registry changes push only an invalidation over the existing websocket', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-fleet-push-'));
  const registry = new RegistryService(join(dir, 'registry.json'));
  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss, { registryService: registry });
  const hello = new Promise<void>((resolve) => wss.once('connection', (socket) => socket.once('message', () => resolve())));
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const ws = new WebSocket(`ws://127.0.0.1:${(server.address() as AddressInfo).port}`);
  try {
    ws.once('open', () => ws.send(JSON.stringify({ type: 'client.hello', clientVersion: 'test', capabilities: {} })));
    await hello;
    const changed = new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('No fleet invalidation received')), 2000);
      ws.on('message', (bytes) => {
        const message = JSON.parse(bytes.toString());
        if (message.type === 'fleet.changed') { clearTimeout(timer); resolve(message); }
      });
    });
    registry.registerNode({ node_id: 'hidden-node-id', display_name: 'Fleet Worker', advertised_url: 'http://127.0.0.1:9',
      version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:PRIVATE_CANARY' });
    assert.deepEqual(await changed, { type: 'fleet.changed' });
  } finally {
    ws.terminate();
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(dir, { recursive: true });
  }
});


test('worker websocket authenticates execution handshake and rejects manager chat', { timeout: 5000 }, async () => {
  const previousToken = process.env.COORDINATOR_TOKEN;
  process.env.COORDINATOR_TOKEN = 'worker-integration-token';
  try { await withWsServer(async (wsUrl) => {
    await assert.rejects(connectAndAwaitWelcome(wsUrl), /401/);
    const ws = await connectAndAwaitWelcome(wsUrl, 'worker-integration-token');
    try {
      const rejected = new Promise<{ type: string; error: string }>((resolve) => ws.once('message', (data) => resolve(JSON.parse(data.toString()))));
      ws.send(JSON.stringify({ type: 'manager.chat.sessionList', profile: 'gah' }));
      const response = await rejected;
      assert.equal(response.type, 'error');
      assert.match(response.error, /only available on the central node/);
    } finally { ws.close(); }
  }, { role: 'worker', central_url: 'https://central.test' });
  } finally {
    if (previousToken === undefined) delete process.env.COORDINATOR_TOKEN; else process.env.COORDINATOR_TOKEN = previousToken;
  }
});

test('trusted-LAN sockets receive a warning but cannot invoke session mutations', async (t) => {
  const { getFleetDispatch } = await import('./wsServer.js');
  const savedMode = process.env.GAH_WS_AUTH_MODE;
  const savedHttp = process.env.GAH_ALLOW_INSECURE_HTTP;
  process.env.GAH_WS_AUTH_MODE = 'trusted_lan';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  const server = http.createServer();
  const wss = createAuthorizedWebSocketServer(server, 'central');
  createWebSocketHandler(wss);
  const fleet = getFleetDispatch();
  const start = t.mock.method(fleet, 'startSession', async () => { throw new Error('must not start'); });
  const stop = t.mock.method(fleet, 'stopSession', async () => { throw new Error('must not stop'); });
  const send = t.mock.method(fleet, 'sendCommand', async () => { throw new Error('must not send'); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`, { headers: { host: `192.168.1.25:${port}`, origin: `http://192.168.1.25:${port}` } });
  try {
    const welcome = await nextWelcome(ws);
    assert.equal(welcome.trustedLanMode, true);
    for (const type of ['session.start', 'session.stop', 'session.sendCommand']) {
      const reply = new Promise<ServerMessage>(resolve => ws.once('message', data => resolve(JSON.parse(data.toString()))));
      ws.send(JSON.stringify({ type, requestId: type, profile: 'gah', nodeId: 'remote', coordinatorNodeId: 'spoof', sessionId: 'remote-session', command: 'touch forbidden' }));
      const response = await reply;
      assert.equal(response.type, 'error');
      assert.match(JSON.stringify(response), /coordinator token is required/);
    }
    assert.equal(start.mock.callCount(), 0);
    assert.equal(stop.mock.callCount(), 0);
    assert.equal(send.mock.callCount(), 0);
  } finally {
    ws.terminate();
    await new Promise<void>(resolve => wss.close(() => resolve()));
    await new Promise<void>(resolve => server.close(() => resolve()));
    if (savedMode === undefined) delete process.env.GAH_WS_AUTH_MODE; else process.env.GAH_WS_AUTH_MODE = savedMode;
    if (savedHttp === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP; else process.env.GAH_ALLOW_INSECURE_HTTP = savedHttp;
  }
});
