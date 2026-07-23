import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { WebSocket, WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
import type { ServerMessage } from '@git-agent-harness/contracts';

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
async function withWsServer(testFn: (wsUrl: string) => Promise<void>) {
  const server = http.createServer();
  const wss = new WebSocketServer({ server });
  createWebSocketHandler(wss);

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

async function connectAndAwaitWelcome(wsUrl: string): Promise<WebSocket> {
  const ws = new WebSocket(wsUrl);
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
