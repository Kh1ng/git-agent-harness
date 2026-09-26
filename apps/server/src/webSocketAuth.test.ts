import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AddressInfo } from 'node:net';
import { WebSocket } from 'ws';
import { DeviceAccess, DEVICE_LIFETIME } from './deviceAccess.js';
import { createAuthorizedWebSocketServer } from './webSocketAuth.js';

test('device expiry terminates its live socket without disconnecting an owner socket', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const directory = mkdtempSync(join(tmpdir(), 'gah-ws-expiry-'));
  const savedToken = process.env.COORDINATOR_TOKEN;
  const savedHttp = process.env.GAH_ALLOW_INSECURE_HTTP;
  process.env.COORDINATOR_TOKEN = 'owner-token';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  let now = Date.now();
  const access = new DeviceAccess(join(directory, 'devices.json'), () => now);
  const server = http.createServer();
  const wss = createAuthorizedWebSocketServer(server, 'central', access, () => now);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as AddressInfo).port;
  const origin = `http://127.0.0.1:${port}`;
  const offer = access.create({ id: 'central', name: 'Central', origin });
  const paired = access.redeem(offer.code, 'central', origin, 'Phone');
  const connect = (headers: Record<string, string>) => new Promise<WebSocket>((resolve, reject) => {
    const socket = new WebSocket(`ws://127.0.0.1:${port}/ws`, ['gah.v1'], { headers });
    socket.once('open', () => resolve(socket));
    socket.once('error', reject);
  });

  let device: WebSocket | undefined;
  let owner: WebSocket | undefined;
  try {
    device = await connect({ Origin: origin, Cookie: `gah_device=${paired.token}` });
    owner = await connect({ Origin: origin, Authorization: 'Bearer owner-token' });

    // Half of the device lifetime has not passed: both sockets stay open,
    // so the timer below targets the expiry, not any firing.
    now += DEVICE_LIFETIME / 2;
    t.mock.timers.tick(2_147_483_647);
    assert.equal(device.readyState, WebSocket.OPEN);

    now += DEVICE_LIFETIME / 2;
    t.mock.timers.tick(2_147_483_647);
    // Bounded wait: a regression must fail red, not hang the suite.
    await new Promise<void>((resolve, reject) => {
      let turns = 0;
      const step = () => {
        if (device!.readyState === WebSocket.CLOSED) resolve();
        else if (++turns > 500) reject(new Error('device socket did not close at expiry'));
        else setImmediate(step);
      };
      setImmediate(step);
    });

    assert.equal(device.readyState, WebSocket.CLOSED);
    assert.equal(owner.readyState, WebSocket.OPEN);
  } finally {
    device?.terminate();
    owner?.terminate();
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>(resolve => wss.close(() => resolve()));
    await new Promise<void>(resolve => server.close(() => resolve()));
    t.mock.timers.reset();
    if (savedToken === undefined) delete process.env.COORDINATOR_TOKEN;
    else process.env.COORDINATOR_TOKEN = savedToken;
    if (savedHttp === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP;
    else process.env.GAH_ALLOW_INSECURE_HTTP = savedHttp;
    rmSync(directory, { recursive: true, force: true });
  }
});

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
