import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer, request as httpRequest } from 'node:http';
import express from 'express';
import cors from 'cors';
import { windowsSetupCommand, nodeSetupRouter, isSupportedWindowsInstaller } from './nodeSetup.js';
import { authMiddleware } from './authMiddleware.js';

test('Windows setup preserves quoted values and rejects unusable origins and roles', () => {
  const command = windowsSetupCommand('http://192.168.1.10:3773', 'both', "a'b$token");
  assert.ok(command.includes("$t='a''b$token'"));
  assert.ok(command.includes('-Role \'both\''));
  assert.ok(command.includes('Authorization="Bearer $t"'));
  for (const url of ['http://localhost:3773', 'http://127.2.3.4', 'file:///tmp/install', 'https://user:secret@central.test', 'https://central.test/path', 'https://central.test/?token=x']) {
    assert.throws(() => windowsSetupCommand(url, 'both', 'token'), Error, url);
  }
  assert.throws(() => windowsSetupCommand('https://central.test', 'arbitrary', 'token'));
  assert.throws(() => windowsSetupCommand('https://central.test', 'desktop', ''));
});

test('setup refuses unsupported worker transport and keeps credential responses out of caches', async () => {
  const previous = { token: process.env.COORDINATOR_TOKEN, insecure: process.env.GAH_ALLOW_INSECURE_HTTP };
  process.env.COORDINATOR_TOKEN = 'test-enrollment-token';
  delete process.env.GAH_ALLOW_INSECURE_HTTP;
  const app = express();
  app.use(express.json());
  app.use('/api/settings/nodes', authMiddleware, nodeSetupRouter());
  const server = createServer(app);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const address = server.address();
    assert.ok(address && typeof address !== 'string');
    const url = `http://127.0.0.1:${address.port}/api/settings/nodes/command`;
    const request = (role: string) => fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ role, centralUrl: 'https://central.test' }) });
    assert.equal((await request('both')).status, 409);
    const desktop = await request('desktop');
    assert.equal(desktop.status, 200);
    assert.equal(desktop.headers.get('cache-control'), 'no-store');
    const body = await desktop.json() as { command: string };
    assert.match(body.command, /test-enrollment-token/);
    process.env.GAH_ALLOW_INSECURE_HTTP = '1';
    assert.equal((await request('both')).status, 200);
  } finally {
    await new Promise<void>((resolve, reject) => server.close((err) => err ? reject(err) : resolve()));
    if (previous.token === undefined) delete process.env.COORDINATOR_TOKEN; else process.env.COORDINATOR_TOKEN = previous.token;
    if (previous.insecure === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP; else process.env.GAH_ALLOW_INSECURE_HTTP = previous.insecure;
  }
});

test('Windows enrollment excludes CLI executables and the broken hidden-window desktop release', () => {
  for (const name of ['gah.exe', 'GAH.Worker_0.1.0_x64-setup.exe', 'GAH.Worker_0.1.1_arm64-setup.exe']) assert.equal(isSupportedWindowsInstaller(name), false);
  for (const name of ['GAH.Worker_0.1.1_x64-setup.exe', 'GAH.Worker_0.2.0_x64-setup.exe', 'GAH.Worker_1.0.0_x64-setup.exe']) assert.equal(isSupportedWindowsInstaller(name), true);
});


test('setup rate limits installer reads and archive generation through one shared boundary', async () => {
  const app = express();
  app.use('/setup', nodeSetupRouter());
  const server = createServer(app);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const address = server.address();
    assert.ok(address && typeof address !== 'string');
    const base = `http://127.0.0.1:${address.port}/setup`;
    for (let attempt = 0; attempt < 30; attempt++) {
      const response = await fetch(`${base}/install.ps1`);
      assert.equal(response.status, 200);
      await response.text();
    }
    for (const path of ['install.ps1', 'source.tar.gz', 'release/linux-cli']) {
      const response = await fetch(`${base}/${path}`);
      assert.equal(response.status, 429);
      assert.ok(Number(response.headers.get('retry-after')) > 0);
      assert.equal(response.headers.get('cache-control'), 'no-store');
      await response.text();
    }
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});


test('setup credentials require auth through proxies, cross-origin browsers, and rebound hosts', async () => {
  const previous = { token: process.env.COORDINATOR_TOKEN, insecure: process.env.GAH_ALLOW_INSECURE_HTTP };
  const token = 'test-setup-origin-token';
  process.env.COORDINATOR_TOKEN = token;
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  const app = express();
  app.set('trust proxy', 'loopback');
  app.use(cors());
  app.use(express.json());
  app.use('/api/settings/nodes', authMiddleware, nodeSetupRouter());
  const server = createServer(app);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const address = server.address();
    assert.ok(address && typeof address !== 'string');
    const origin = `http://127.0.0.1:${address.port}`;
    const request = (headers: Record<string, string>, path = 'command') => new Promise<{ status: number | undefined; body: string }>((resolve, reject) => {
      // fetch overwrites Host; use the HTTP transport to exercise rebinding.
      const outgoing = httpRequest(`${origin}/api/settings/nodes/${path}`, {
        method: path === 'command' ? 'POST' : 'GET',
        headers: { 'Content-Type': 'application/json', ...headers },
      }, (response) => {
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => { body += chunk; });
        response.on('end', () => resolve({ status: response.statusCode, body }));
        response.on('error', reject);
      });
      outgoing.on('error', reject);
      outgoing.end(path === 'command' ? JSON.stringify({ role: 'desktop', centralUrl: 'https://central.test' }) : undefined);
    });
    // Preserve local CLI and the locally hosted native/browser dashboard.
    const trusted: Record<string, string>[] = [{}, { Origin: origin }, { Host: 'localhost', Origin: 'http://localhost' }];
    for (const headers of trusted) {
      const response = await request(headers);
      assert.equal(response.status, 200);
      assert.match(response.body, /test-setup-origin-token/);
    }
    const untrusted: Record<string, string>[] = [
      { 'X-Forwarded-For': '198.51.100.50', 'X-Forwarded-Proto': 'https' },
      { Forwarded: 'for=198.51.100.50;proto=https' },
      { 'X-Forwarded-Host': 'central.example' },
      { 'X-Forwarded-Port': '443' },
      { 'X-Forwarded-For': '127.0.0.1' },
      { Origin: 'https://untrusted.example' },
      { Origin: 'null' },
      { Origin: 'http://127.0.0.1:1' },
      { Host: 'rebound.example', Origin: 'http://rebound.example' },
      { Host: 'rebound.example' },
      { Host: '127.rebound.example', Origin: 'http://127.rebound.example' },
      { Host: '127.rebound.example' },
      { Host: 'localhost.rebound.example' },
    ];
    for (const headers of untrusted) {
      const response = await request(headers);
      assert.equal(response.status, 401, JSON.stringify(headers));
      assert.ok(!(response.body).includes(token));
    }
    // A valid bearer token permits each transport; an incorrect one does not.
    for (const headers of untrusted) {
      const authenticated = await request({ ...headers, Authorization: `Bearer ${token}` });
      assert.equal(authenticated.status, 200, JSON.stringify(headers));
      assert.match(authenticated.body, /test-setup-origin-token/);
    }
    const invalid = await request({ ...untrusted[0], Authorization: 'Bearer incorrect-token' });
    assert.equal(invalid.status, 401);
    assert.ok(!invalid.body.includes(token));
    // The wildcard CORS response cannot turn an unauthenticated browser read
    // into credential access, including an originless GET after DNS rebinding.
    const reboundRead = await request({ Host: 'rebound.example' }, 'install.ps1');
    assert.equal(reboundRead.status, 401);
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    if (previous.token === undefined) delete process.env.COORDINATOR_TOKEN; else process.env.COORDINATOR_TOKEN = previous.token;
    if (previous.insecure === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP; else process.env.GAH_ALLOW_INSECURE_HTTP = previous.insecure;
  }
});
