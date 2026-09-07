import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import express from 'express';
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
