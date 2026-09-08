import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { WebSocket } from 'ws';
import { createAuthorizedWebSocketServer } from './webSocketAuth.js';

test('real upgrade authenticates before welcome or mutation handlers can run', async () => {
  const keys = ['COORDINATOR_TOKEN', 'GAH_ALLOW_INSECURE_HTTP', 'GAH_WS_AUTH_MODE', 'GAH_NODE_ROLE', 'GAH_WS_ALLOWED_ORIGINS'] as const;
  const saved = keys.map(key => process.env[key]);
  process.env.COORDINATOR_TOKEN = 'test-secret';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  delete process.env.GAH_WS_AUTH_MODE;
  delete process.env.GAH_NODE_ROLE;
  delete process.env.GAH_WS_ALLOWED_ORIGINS;
  const server = http.createServer();
  const wss = createAuthorizedWebSocketServer(server);
  let connections = 0;
  let mutations = 0;
  wss.on('connection', ws => {
    connections++;
    ws.send('welcome');
    ws.on('message', () => { mutations++; });
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  const url = `ws://127.0.0.1:${port}/ws`;
  async function connect(headers: Record<string, string>, protocols: string[] = []): Promise<WebSocket | number> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url, protocols, { headers });
      ws.once('open', () => resolve(ws));
      ws.once('unexpected-response', (_req, res) => { res.resume(); resolve(res.statusCode!); });
      ws.once('error', reject);
    });
  }
  async function allowed(headers: Record<string, string>, protocols: string[] = []) {
    const ws = await connect(headers, protocols);
    assert.ok(ws instanceof WebSocket);
    assert.ok(!ws.protocol.includes('test-secret'));
    const protocol = ws.protocol;
    ws.close();
    await new Promise<void>(resolve => ws.once('close', () => resolve()));
    return protocol;
  }
  const remote = { host: `192.168.1.25:${port}`, origin: `http://192.168.1.25:${port}` };
  try {
    assert.equal(await connect(remote), 401);
    assert.equal(connections, 0);
    assert.equal(mutations, 0);
    assert.equal(await connect({ ...remote, authorization: 'Bearer wrong' }), 401);
    await allowed({ ...remote, authorization: 'Bearer test-secret' });
    assert.equal(await allowed(remote, ['gah.v1', `gah-auth.${Buffer.from('test-secret').toString('base64url')}`]), 'gah.v1');
    await allowed({ host: `localhost:${port}`, origin: `http://localhost:${port}` });
    await allowed({ authorization: 'Bearer test-secret' });
    assert.equal(await connect({ ...remote, origin: 'https://evil.example', authorization: 'Bearer test-secret' }), 403);
    assert.equal(await connect({ host: `evil.example:${port}`, origin: `http://evil.example:${port}` }), 403);
    assert.equal(await connect({ ...remote, origin: 'null' }), 403);
    delete process.env.GAH_ALLOW_INSECURE_HTTP;
    assert.equal(await connect({ ...remote, authorization: 'Bearer test-secret' }), 403);
    // Only the immediate local proxy can attest TLS; it does not grant local auth.
    assert.equal(await connect({ ...remote, origin: `https://192.168.1.25:${port}`, 'x-forwarded-proto': 'https' }), 401);
    await allowed({ ...remote, origin: `https://192.168.1.25:${port}`, 'x-forwarded-proto': 'https', authorization: 'Bearer test-secret' });
    const named = { host: 'gah.example.test', origin: 'https://gah.example.test', 'x-forwarded-proto': 'https', authorization: 'Bearer test-secret' };
    assert.equal(await connect(named), 403);
    process.env.GAH_WS_ALLOWED_ORIGINS = 'https://gah.example.test,http://localhost:3000';
    await allowed(named);
    await allowed({ ...named, origin: 'http://localhost:3000' });
    process.env.GAH_ALLOW_INSECURE_HTTP = '1';
    process.env.GAH_WS_AUTH_MODE = 'trusted_lan';
    assert.equal(await connect({ ...remote, authorization: 'Bearer wrong' }), 401);
    assert.equal(await connect(remote, ['gah.v1', 'gah-auth.invalid']), 401);
    assert.equal(await connect(remote, ['gah.v1', 'gah-auth.%%%']), 401);
    assert.equal(await connect(remote, ['gah.v1', 'gah-auth.Zm9v', 'gah-auth.YmFy']), 401);
    await allowed(remote);
    assert.equal(await connect({ host: remote.host }), 401);
    process.env.GAH_NODE_ROLE = 'worker';
    assert.equal(await connect(remote), 401);
    assert.equal(await connect({ host: `localhost:${port}` }), 401);
    delete process.env.GAH_ALLOW_INSECURE_HTTP;
    await allowed({ authorization: 'Bearer test-secret' });
  } finally {
    for (const ws of wss.clients) ws.terminate();
    await new Promise<void>(resolve => wss.close(() => resolve()));
    await new Promise<void>(resolve => server.close(() => resolve()));
    keys.forEach((key, i) => saved[i] === undefined ? delete process.env[key] : process.env[key] = saved[i]);
  }
});


test('a remote peer cannot claim TLS through forwarded headers', () => {
  const savedHttp = process.env.GAH_ALLOW_INSECURE_HTTP;
  const savedToken = process.env.COORDINATOR_TOKEN;
  delete process.env.GAH_ALLOW_INSECURE_HTTP;
  process.env.COORDINATOR_TOKEN = 'tls-test';
  const server = http.createServer();
  const wss = createAuthorizedWebSocketServer(server, 'worker');
  try {
    const verify = wss.options.verifyClient;
    assert.equal(typeof verify, 'function');
    let status: number | undefined;
    Reflect.apply(verify!, wss, [{ req: { socket: { remoteAddress: '198.51.100.25' }, headers: {
      host: '192.168.1.25:3773', origin: 'https://192.168.1.25:3773',
      'x-forwarded-proto': 'https', authorization: 'Bearer tls-test'
    } } }, (allowed: boolean, code?: number) => { assert.equal(allowed, false); status = code; }]);
    assert.equal(status, 403);
    // A persisted worker role must require auth even on a direct local socket.
    Reflect.apply(verify!, wss, [{ req: { socket: { remoteAddress: '127.0.0.1' }, headers: { host: 'localhost:3773' } } },
      (allowed: boolean, code?: number) => { assert.equal(allowed, false); assert.equal(code, 401); }]);
  } finally {
    wss.close();
    if (savedHttp === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP; else process.env.GAH_ALLOW_INSECURE_HTTP = savedHttp;
    if (savedToken === undefined) delete process.env.COORDINATOR_TOKEN; else process.env.COORDINATOR_TOKEN = savedToken;
  }
});
