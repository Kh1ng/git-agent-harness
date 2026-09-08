import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import type { AddressInfo } from 'node:net';
import { WebSocket } from 'ws';
import type { PairingOffer, PairingPreview, PairedDevice } from '@git-agent-harness/contracts';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import { createServer } from './server.js';
import { createAuthorizedWebSocketServer, webSocketAccessValid } from './webSocketAuth.js';
import { COORDINATOR_SCHEMA_DIGEST, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { RegistryService } from './registryService.js';
import { FIXTURE_GAH_BINARY } from './fixtureGahHarness.js';

test('real HTTP/WS pairing confirms access, rejects CSRF and owner exports, and revokes existing sockets individually', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-pairing-http-'));
  const keys = ['COORDINATOR_TOKEN', 'GAH_ALLOW_INSECURE_HTTP', 'GAH_COORDINATOR_IDENTITY_PATH', 'GAH_WS_AUTH_MODE',
    'GAH_GATEWAY_SETTINGS_PATH', 'GAH_MANAGER_CHAT_SETTINGS_PATH', 'GAH_ENABLE_ADMIN_UPDATE', 'GAH_BINARY', 'GAH_FIXTURE_PROFILE_LIST'] as const;
  const saved = keys.map(key => process.env[key]);
  process.env.COORDINATOR_TOKEN = 'owner-secret-never-in-qr';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(directory, 'identity.json');
  process.env.GAH_WS_AUTH_MODE = 'trusted_lan';
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(directory, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(directory, 'manager-chat.json');
  process.env.GAH_ENABLE_ADMIN_UPDATE = '1';
  process.env.GAH_BINARY = FIXTURE_GAH_BINARY;
  process.env.GAH_FIXTURE_PROFILE_LIST = join(directory, 'profiles.json');
  writeFileSync(process.env.GAH_FIXTURE_PROFILE_LIST, '[]');
  const gatewaySettings = JSON.stringify({ url: 'https://gateway.example.test', apiKey: 'private-gateway-key', enabled: true });
  const managerSettings = JSON.stringify({ defaultBackend: 'claude', profileOverrides: {} });
  writeFileSync(process.env.GAH_GATEWAY_SETTINGS_PATH, gatewaySettings);
  writeFileSync(process.env.GAH_MANAGER_CHAT_SETTINGS_PATH, managerSettings);
  resetCachedCoordinatorIdentity();
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const registry = new RegistryService(join(directory, 'registry.json'));
  const registeredNode = { node_id: 'paired-test-worker', display_name: 'Worker', advertised_url: 'http://127.0.0.1:3999',
    version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback' as const, secret_ref: 'env:WORKER_TOKEN' };
  registry.registerNode(registeredNode);
  const originalRegistry = JSON.stringify(registry.getNodes());
  let adminUpdates = 0;
  const server = http.createServer(createServer({ deviceAccess: access, registryService: registry, detectTailscaleIPv4: async () => null,
    startAdminUpdate: () => {
      adminUpdates++;
      return { started: true, state: { status: 'running', startedAt: '2026-09-08T00:00:00Z', finishedAt: null, exitCode: null, pid: 1234, output: '' } };
    } }));
  const wss = createAuthorizedWebSocketServer(server, 'central', access);
  let messages = 0;
  let deviceSocket: WebSocket | undefined;
  wss.on('connection', ws => {
    deviceSocket = ws;
    ws.on('message', () => { if (webSocketAccessValid(ws)) messages++; });
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const remote = { 'X-Forwarded-For': '198.51.100.2', Origin: base };
  const owner = { ...remote, Authorization: 'Bearer owner-secret-never-in-qr' };
  const post = (path: string, body: unknown, headers: Record<string, string> = remote) => fetch(base + path, { method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const connect = (cookie: string, origin: string | null = base): Promise<WebSocket | number> => new Promise((resolve, reject) => {
    const socket = new WebSocket(base.replace('http:', 'ws:') + '/ws', ['gah.v1'], { headers: {
      'X-Forwarded-For': remote['X-Forwarded-For'], Cookie: cookie, ...(origin === null ? { Referer: base + '/' } : { Origin: origin })
    } });
    socket.once('open', () => resolve(socket));
    socket.once('unexpected-response', (_req, res) => { res.resume(); resolve(res.statusCode!); });
    socket.once('error', reject);
  });
  try {
    assert.equal((await post('/api/pairing/offers', { origin: base })).status, 401);
    const response = await post('/api/pairing/offers', { origin: base }, owner);
    assert.equal(response.status, 200);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    const offer = await response.json() as PairingOffer;
    assert.ok(!JSON.stringify(offer).includes(process.env.COORDINATOR_TOKEN!));
    const input = { code: offer.code, server_id: offer.server.id };
    assert.equal((await post('/api/pairing/inspect', input, { ...remote, Origin: 'http://attacker.test' })).status, 403);
    assert.equal((await post('/api/pairing/inspect', { ...input, server_id: 'wrong-server' })).status, 400);
    const preview = await post('/api/pairing/inspect', input);
    assert.equal(preview.status, 200);
    assert.match((await preview.json() as PairingPreview).access, /run agent work/);
    assert.equal((await post('/api/pairing/redeem', { ...input, name: 'Phone' })).status, 400);
    const redeemed = await post('/api/pairing/redeem', { ...input, name: 'Phone', confirm: true });
    assert.equal(redeemed.status, 200);
    const setCookie = redeemed.headers.get('set-cookie')!;
    assert.match(setCookie, /HttpOnly/);
    assert.match(setCookie, /SameSite=Strict/);
    assert.ok(!setCookie.includes('Secure'), 'Explicit trusted HTTP must not silently issue an unusable Secure cookie');
    const cookie = setCookie.split(';')[0];
    const result = await redeemed.json() as { device: PairedDevice };
    assert.ok(!JSON.stringify(result).includes(cookie.split('=')[1]));
    assert.equal((await post('/api/pairing/redeem', { ...input, name: 'Replay', confirm: true })).status, 400);
    const paired = { ...remote, Cookie: cookie };
    assert.equal((await fetch(base + '/api/info', { headers: paired })).status, 200);
    assert.equal((await fetch(base + '/api/info', { headers: { Cookie: cookie, 'Sec-Fetch-Site': 'same-origin' } })).status, 200, 'Same-origin browser GETs may omit Origin');
    // WebView2 omits Origin and Fetch Metadata on plain-HTTP dashboard reads.
    const httpBrowser = { 'X-Forwarded-For': remote['X-Forwarded-For'], Cookie: cookie, Referer: base + '/' };
    assert.equal((await fetch(base + '/api/pairing/session', { headers: httpBrowser })).status, 200, 'Paired HTTP browser remains authenticated after redeem');
    assert.equal((await fetch(base + '/api/info', { headers: httpBrowser })).status, 200);
    assert.equal((await fetch(base + '/api/info', { method: 'HEAD', headers: httpBrowser })).status, 200);
    for (const headers of [
      { ...httpBrowser, Referer: 'http://attacker.test/' },
      { ...httpBrowser, Referer: base.replace('http:', 'https:') + '/' },
      { ...httpBrowser, Referer: 'not a URL' },
      { ...httpBrowser, Origin: 'http://attacker.test' },
      { ...httpBrowser, Origin: 'null' },
      { ...httpBrowser, 'Sec-Fetch-Site': 'cross-site' },
      { ...httpBrowser, 'Sec-Fetch-Site': 'same-site' },
      { ...httpBrowser, 'Sec-Fetch-Site': 'none' },
    ]) assert.equal((await fetch(base + '/api/info', { headers })).status, 401, 'Referer cannot override absent or conflicting origin evidence');
    assert.equal((await post('/api/config', {}, httpBrowser)).status, 401, 'Mutations still require Origin');
    assert.equal((await post('/api/pairing/logout', {}, httpBrowser)).status, 403, 'Pairing mutations still require Origin');
    assert.equal((await fetch(base + '/api/info', { headers: { Cookie: cookie } })).status, 401, 'Cookie access without browser origin evidence is denied');
    assert.equal((await fetch(base + '/api/info', { headers: { ...paired, Origin: 'http://attacker.test' } })).status, 401);
    assert.equal((await post('/api/pairing/offers', { origin: base }, paired)).status, 403);
    assert.equal((await post('/api/settings/nodes/command', { centralUrl: base, role: 'desktop' }, paired)).status, 403);
    assert.equal((await post('/api/settings/gateway/bootstrap-command', {}, paired)).status, 403);
    const ownerOperations: Array<[string, string, unknown]> = [
      ['POST', '/api/registry/nodes', { ...registeredNode, node_id: 'untrusted-worker' }],
      ['DELETE', `/api/registry/nodes/${registeredNode.node_id}`, {}],
      ['POST', `/api/registry/nodes/${registeredNode.node_id}/rotate-secret`, { secret_ref: 'env:REPLACEMENT_TOKEN' }],
      ['POST', '/api/config', { current_manager: 'claude' }],
      ['POST', '/api/profiles', { name: 'paired-test', display_name: 'Paired test', repo_id: 'paired-test', provider: 'github', repo: 'test/repo', local_path: directory, artifact_root: directory }],
      ['PATCH', '/api/profiles/paired-test', { display_name: 'Changed' }],
      ['DELETE', '/api/profiles/paired-test', {}],
      ['PUT', '/api/settings/gateway', { url: 'https://attacker.example.test' }],
      ['POST', '/api/manager-chat/settings', { defaultBackend: 'codex' }],
      ['POST', '/api/manager-chat/reclaim', { profile: 'paired-test', dryRun: false }],
      ['POST', '/api/admin/update', {}],
    ];
    for (const [method, path, body] of ownerOperations) {
      const denied = await fetch(base + path, { method, headers: { ...paired, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      assert.equal(denied.status, 403, `${method} ${path}`);
      assert.deepEqual(await denied.json(), { error: 'Forbidden', message: 'This operation requires owner access.' });
    }
    assert.equal(JSON.stringify(registry.getNodes()), originalRegistry, 'Paired requests cannot add, revoke, or change worker credentials');
    assert.equal(readFileSync(process.env.GAH_GATEWAY_SETTINGS_PATH, 'utf8'), gatewaySettings, 'Paired requests cannot redirect the retained gateway key');
    assert.equal(readFileSync(process.env.GAH_MANAGER_CHAT_SETTINGS_PATH, 'utf8'), managerSettings);
    assert.equal(adminUpdates, 0, 'Denied admin requests never start an update');
    assert.equal((await post('/api/admin/update', {}, owner)).status, 202);
    assert.equal(adminUpdates, 1, 'Explicit owner credentials retain administration access');
    assert.equal((await fetch(base + '/api/info', { headers: { ...paired, Cookie: `${cookie}; ${cookie}` } })).status, 401);
    assert.equal(await connect(cookie, 'http://attacker.test'), 403);
    assert.equal(await connect(cookie, null), 401, 'Referer alone cannot authorize a WebSocket upgrade');
    const socket = await connect(cookie);
    assert.ok(socket instanceof WebSocket);
    socket.send('test');
    await new Promise<void>(resolve => deviceSocket!.once('message', () => resolve()));
    assert.equal(messages, 1);
    const closed = new Promise<void>(resolve => socket.once('close', () => resolve()));
    assert.equal((await fetch(`${base}/api/pairing/devices/${result.device.id}`, { method: 'DELETE', headers: owner })).status, 200);
    await closed;
    assert.equal(webSocketAccessValid(deviceSocket!), false);
    assert.equal((await fetch(base + '/api/info', { headers: paired })).status, 401);
    assert.equal(await connect(cookie), 401, 'A revoked cookie cannot downgrade into trusted-LAN mode');
    assert.equal((await fetch(base + '/api/info', { headers: { Cookie: cookie, Origin: base } })).status, 401, 'Local trust cannot rescue revoked supplied credentials');
    const logout = await post('/api/pairing/logout', {}, paired);
    assert.equal(logout.status, 200);
    assert.match(logout.headers.get('set-cookie')!, new RegExp(`${DEVICE_COOKIE}=;`));
    // TLS proxy transport sets a Secure cookie; disabling HTTP prevents insecure offers.
    delete process.env.GAH_ALLOW_INSECURE_HTTP;
    assert.equal((await post('/api/pairing/offers', { origin: base }, owner)).status, 403);
    const tlsOrigin = base.replace('http:', 'https:');
    const tlsHeaders = { ...remote, Origin: tlsOrigin, 'X-Forwarded-Proto': 'https' };
    const tlsOffer = await (await post('/api/pairing/offers', { origin: tlsOrigin }, { ...tlsHeaders, Authorization: 'Bearer owner-secret-never-in-qr' })).json() as PairingOffer;
    const tlsResult = await post('/api/pairing/redeem', { code: tlsOffer.code, server_id: tlsOffer.server.id, name: 'Secure phone', confirm: true }, tlsHeaders);
    assert.equal(tlsResult.status, 200);
    assert.match(tlsResult.headers.get('set-cookie')!, /; Secure/);
    assert.equal((await post('/api/pairing/offers', { origin: 'https://unconfigured.example.test' }, { ...tlsHeaders, Authorization: 'Bearer owner-secret-never-in-qr' })).status, 400);
    for (let i = 0; i < 35; i++) await post('/api/pairing/inspect', input, tlsHeaders);
    assert.equal((await post('/api/pairing/inspect', input, tlsHeaders)).status, 429);
  } finally {
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>(resolve => wss.close(() => resolve()));
    await new Promise<void>(resolve => server.close(() => resolve()));
    keys.forEach((key, index) => { if (saved[index] === undefined) delete process.env[key]; else process.env[key] = saved[index]; });
    resetCachedCoordinatorIdentity();
    rmSync(directory, { recursive: true, force: true });
  }
});
