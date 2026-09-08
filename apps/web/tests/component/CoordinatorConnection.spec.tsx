import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { CoordinatorConnection } from '../../src/components/CoordinatorConnection.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

test('saving a token reconnects the real browser socket without putting credentials in its URL', async ({ mount, page }, testInfo) => {
  let connections = 0;
  await page.routeWebSocket('**/ws', ws => {
    connections++;
    if (connections === 1) { ws.close(); return; }
    ws.send(JSON.stringify({ type: 'server.welcome', serverVersion: 'test', trustedLanMode: true, serverProviderCatalog: { providers: [] }, sessions: [], providers: {} }));
  });
  await page.reload();
  await page.evaluate(() => {
    sessionStorage.clear();
    const NativeWebSocket = window.WebSocket;
    const attempts: { url: string; protocols?: string | string[] }[] = [];
    Object.assign(window, { __connections: attempts });
    window.WebSocket = class extends NativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        attempts.push({ url: String(url), protocols });
        super(url, protocols);
      }
    };
  });
  const component = await mount(<WebSocketProvider><CoordinatorConnection /></WebSocketProvider>);
  await component.getByLabel('Access token', { exact: true }).fill('browser-secret');
  await component.getByRole('button', { name: 'Save and reconnect' }).click();
  await expect(component.getByRole('status')).toContainText('Trusted-LAN mode is enabled');
  const attempts = await page.evaluate(() => (window as typeof window & { __connections: { url: string; protocols: string[] }[] }).__connections);
  expect(attempts.at(-1)?.protocols).toEqual(['gah.v1', `gah-auth.${Buffer.from('browser-secret').toString('base64url')}`]);
  expect(attempts.every(attempt => !attempt.url.includes('browser-secret') && !attempt.url.includes('gah-auth'))).toBe(true);
  expect(await page.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBe('browser-secret');
  await component.getByText('Central access token', { exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath('connection-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: testInfo.outputPath('connection-mobile.png') });
  await expect(component.getByRole('status')).toBeVisible();
});

test('unavailable browser storage leaves the connection form usable and reports save failure', async ({ mount, page }) => {
  await page.evaluate(() => Object.defineProperty(window, 'sessionStorage', {
    configurable: true,
    get() { throw new DOMException('Storage disabled', 'SecurityError'); }
  }));
  const component = await mount(<WebSocketProvider><CoordinatorConnection /></WebSocketProvider>);
  await component.getByLabel('Access token', { exact: true }).fill('unsaved-token');
  await component.getByRole('button', { name: 'Save and reconnect' }).click();
  await expect(component.getByRole('alert')).toHaveText('Cannot save the token in this tab. Check your browser storage settings.');
});
