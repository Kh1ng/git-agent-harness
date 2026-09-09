import { expect, test } from '@playwright/test';
import express from 'express';
import http from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AddressInfo } from 'node:net';
import { DeviceAccess } from '../../../server/src/deviceAccess.js';
import { pairingRouter } from '../../../server/src/pairing.js';
import { authMiddleware } from '../../../server/src/authMiddleware.js';
import { createAuthorizedWebSocketServer } from '../../../server/src/webSocketAuth.js';
import { createMockControlPlane } from '../../../server/mock/controlPlane.js';

test('QR/manual pairing confirms the server, persists an HttpOnly session, and revokes a live browser', async ({ browser, baseURL }, testInfo) => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-browser-pairing-'));
  const saved = { token: process.env.COORDINATOR_TOKEN, http: process.env.GAH_ALLOW_INSECURE_HTTP };
  process.env.COORDINATOR_TOKEN = 'browser-owner-secret';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const mock = createMockControlPlane();
  const mockRunning = await mock.listen(0);
  const app = express();
  app.set('trust proxy', 'loopback');
  app.locals.deviceAccess = access;
  // Exercise the remote boundary while the hermetic listener runs on loopback.
  app.use((req, _res, next) => { req.headers['x-forwarded-for'] = req.headers['x-test-client'] === 'owner' ? '198.51.100.8' : '198.51.100.9'; next(); });
  app.use(express.json());
  app.use('/api/pairing', pairingRouter(access, { node_id: '11111111-1111-1111-1111-111111111111', display_name: 'Pairing test central', advertised_url: '', version: 'test', schema_digest: 'test' }));
  app.use('/api', authMiddleware);
  app.use((req, res, next) => { if (req.path.startsWith('/api/')) mock.app(req, res, next); else next(); });
  // Serve the same Vite application at this fixture origin. Only auth is real;
  // all dashboard/provider data still belongs to the in-memory mock above.
  app.use((req, res) => {
    const vite = new URL(baseURL!);
    const proxied = http.request({ hostname: vite.hostname, port: vite.port, path: req.url, headers: req.headers }, upstream => {
      res.writeHead(upstream.statusCode ?? 502, upstream.headers);
      upstream.pipe(res);
    });
    proxied.on('error', () => res.sendStatus(502));
    req.pipe(proxied);
  });
  const server = http.createServer(app);
  server.on('upgrade', req => { req.headers['x-forwarded-for'] = '198.51.100.9'; });
  const wss = createAuthorizedWebSocketServer(server, 'central', access);
  wss.on('connection', (ws, req) => mock.wss.emit('connection', ws, req));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const ownerContext = await browser.newContext({ extraHTTPHeaders: { 'x-test-client': 'owner' } });
  const deviceContext = await browser.newContext({ hasTouch: true, viewport: { width: 320, height: 568 } });
  const owner = await ownerContext.newPage();
  const phone = await deviceContext.newPage();
  let closedDeviceSockets = 0;
  phone.on('websocket', socket => { if (new URL(socket.url()).pathname === '/ws') socket.on('close', () => { closedDeviceSockets++; }); });
  try {
    await owner.goto(`${origin}/?page=settings`);
    await owner.getByLabel('Access token', { exact: true }).fill('browser-owner-secret');
    await owner.getByRole('button', { name: 'Save and reconnect' }).click();
    await owner.getByRole('button', { name: 'Pair a device', exact: true }).click();
    await owner.clock.install();
    await owner.getByRole('button', { name: 'Generate pairing QR code' }).click();
    const expiredLink = await owner.getByLabel('Pairing link', { exact: true }).inputValue();
    await owner.clock.fastForward(301_000);
    await expect(owner.getByRole('img', { name: 'Scan to pair with this central server' })).toHaveCount(0);
    await expect(owner.getByText('Pairing code expired. Generate a new code.')).toBeVisible();
    await owner.clock.setSystemTime(new Date());
    await owner.getByRole('button', { name: 'Generate pairing QR code' }).click();
    const link = await owner.getByLabel('Pairing link', { exact: true }).inputValue();
    expect(link).not.toBe(expiredLink);
    expect(link).not.toContain('browser-owner-secret');
    await expect(owner.getByRole('img', { name: 'Scan to pair with this central server' })).toBeVisible();
    await expect(owner.getByText('In the GAH iPhone app, open Connection, then Scan pairing QR code.')).toBeVisible();
    await expect(owner.getByText('To pair a browser, open the pairing link there. The iPhone Camera app opens Safari; pairing there signs in Safari only.')).toBeVisible();
    await phone.goto(link);
    await expect(phone.getByRole('heading', { name: 'Confirm this server' })).toBeVisible();
    expect(new URL(phone.url()).searchParams.get('page')).toBe('settings');
    await expect(phone.getByText(`Pairing test central · ${origin}`, { exact: true })).toBeVisible();
    await expect(phone.getByText(/Dashboard control: read projects and chats, run agent work/)).toBeVisible();
    await expect(phone.getByText(/permits unencrypted HTTP/)).toBeVisible();
    expect(new URL(phone.url()).hash).toBe('');
    expect((await deviceContext.cookies()).some(cookie => cookie.name === 'gah_device')).toBe(false);
    const pairing = phone.getByRole('region', { name: 'Device pairing' });
    for (const viewport of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }]) {
      await phone.setViewportSize(viewport);
      await expect.poll(() => pairing.locator('button, input').evaluateAll(elements => elements.every(element => element.getBoundingClientRect().height >= 44))).toBe(true);
      await expect(phone.getByLabel('Device name', { exact: true })).toHaveCSS('font-size', '16px');
      expect(await phone.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await phone.screenshot({ path: testInfo.outputPath(`pairing-confirm-${viewport.width}.png`) });
    }
    await phone.setViewportSize({ width: 320, height: 568 });
    await phone.getByLabel('Device name', { exact: true }).fill('Test phone');
    await phone.getByRole('button', { name: 'Confirm server and pair' }).click();
    await expect(phone.getByRole('status').filter({ hasText: 'Paired as Test phone' })).toBeVisible();
    const cookie = (await deviceContext.cookies()).find(cookie => cookie.name === 'gah_device');
    expect(cookie).toMatchObject({ httpOnly: true, sameSite: 'Strict', secure: false });
    expect(cookie!.expires).toBeGreaterThan(Date.now() / 1000);
    expect(await phone.evaluate(() => document.cookie)).not.toContain('gah_device');
    expect(await phone.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBeNull();
    await phone.reload();
    await expect(phone.getByRole('status').filter({ hasText: 'Paired device · Dashboard access enabled' })).toBeVisible();
    await expect(phone.getByRole('button', { name: 'Pair this device', exact: true })).toHaveCount(0);
    await phone.getByRole('button', { name: 'Manage pairing', exact: true }).click();
    await expect(phone.getByText('Pairing stays signed in across app or browser restarts until it expires or the owner revokes access.')).toBeVisible();
    await expect(phone.getByRole('button', { name: 'Generate pairing QR code' })).toHaveCount(0);
    expect(await phone.evaluate(async () => (await fetch('/api/pairing/offers', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ origin: location.origin }) })).status)).toBe(403);
    await phone.getByRole('button', { name: 'Pair another device', exact: true }).click();
    await expect(phone.getByLabel('Access token', { exact: true })).toBeFocused();
    await expect(phone.getByLabel('Access token', { exact: true })).toHaveValue('');
    await expect(phone.getByText('Owner access is required to generate pairing QR codes and administer central. Paired devices can use the dashboard without this token. Stored only for this tab’s session.')).toBeVisible();
    expect(await phone.evaluate(async () => (await (await fetch('/api/pairing/session')).json()).principal.kind)).toBe('device');
    for (const width of [320, 1280]) {
      await phone.setViewportSize({ width, height: 844 });
      expect(await phone.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await phone.screenshot({ path: testInfo.outputPath(`paired-device-${width}.png`) });
    }
    expect(await phone.evaluate(async () => (await fetch('/api/profiles')).status)).toBe(200);
    await owner.getByRole('button', { name: 'Refresh devices' }).click();
    await expect(owner.getByRole('button', { name: 'Revoke Test phone', exact: true })).toBeVisible();
    await owner.getByRole('heading', { name: 'Paired devices' }).scrollIntoViewIfNeeded();
    await owner.screenshot({ path: testInfo.outputPath('paired-devices-desktop.png') });
    // Owner access is explicit and temporary; clearing it returns to the paired device.
    await phone.getByLabel('Access token', { exact: true }).fill('browser-owner-secret');
    await phone.getByRole('button', { name: 'Save and reconnect' }).click();
    await expect(phone.getByRole('button', { name: 'Generate pairing QR code' })).toBeVisible();
    await phone.getByRole('button', { name: 'Generate pairing QR code' }).click();
    await expect(phone.getByRole('img', { name: 'Scan to pair with this central server' })).toBeVisible();
    await phone.getByText('Central access token', { exact: true }).click();
    await phone.getByLabel('Access token', { exact: true }).fill('');
    await phone.getByRole('button', { name: 'Save and reconnect' }).click();
    await expect(phone.getByRole('status').filter({ hasText: 'Paired device · Dashboard access enabled' })).toBeVisible();
    const closedBeforeRevocation = closedDeviceSockets;
    await owner.getByRole('button', { name: 'Revoke Test phone', exact: true }).click();
    await expect(owner.getByRole('status').filter({ hasText: 'Revoked Test phone' })).toBeVisible();
    await expect.poll(() => phone.evaluate(async () => (await fetch('/api/profiles')).status)).toBe(401);
    await expect.poll(() => closedDeviceSockets).toBeGreaterThan(closedBeforeRevocation);
    // The manual paste fallback must reject the already-used code as well.
    await phone.goto(`${origin}/?page=settings`);
    await phone.getByRole('button', { name: 'Pair this device', exact: true }).click();
    await expect(phone.getByText('Ask the owner for a pairing link or QR code to stay signed in on this device. An owner access token grants temporary access for this tab only.')).toBeVisible();
    await phone.getByLabel('Open a pairing link', { exact: true }).fill(link);
    await phone.getByRole('button', { name: 'Review pairing link' }).click();
    await expect(phone.getByRole('alert').filter({ hasText: 'already used' })).toBeVisible();
    await expect(phone.getByRole('button', { name: 'Confirm server and pair' })).toHaveCount(0);
  } finally {
    await ownerContext.close(); await deviceContext.close();
    for (const socket of wss.clients) socket.terminate();
    await new Promise<void>(resolve => wss.close(() => resolve()));
    await new Promise<void>(resolve => server.close(() => resolve()));
    await mockRunning.close();
    if (saved.token === undefined) delete process.env.COORDINATOR_TOKEN; else process.env.COORDINATOR_TOKEN = saved.token;
    if (saved.http === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP; else process.env.GAH_ALLOW_INSECURE_HTTP = saved.http;
    rmSync(directory, { recursive: true, force: true });
  }
});
