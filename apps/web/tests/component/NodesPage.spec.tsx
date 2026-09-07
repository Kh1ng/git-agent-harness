import { expect, test } from '@playwright/experimental-ct-react';
import type { Page } from '@playwright/test';
import React from 'react';
import { NodesPage } from '../../src/pages/NodesPage.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

async function connectSocket(page: Page) {
  await page.evaluate(() => {
    class TestSocket {
      static OPEN = 1;
      static CONNECTING = 0;
      readyState = 1;
      onopen: ((event: Event) => void) | null = null;
      onmessage: ((event: MessageEvent) => void) | null = null;
      onclose = null;
      onerror = null;
      constructor() {
        Object.assign(window, { fleetSocket: this });
        queueMicrotask(() => this.onopen?.(new Event('open')));
      }
      send() {}
      close() {}
    }
    window.WebSocket = TestSocket as unknown as typeof WebSocket;
  });
}

const node = (id: string) => ({ node_id: id, display_name: `Worker ${id}`, advertised_url: 'http://192.168.1.20:3773',
  transport_mode: 'trusted_lan', profiles: ['gah'], version: '0.1.0', schema_digest: 'fixture' });
const observation = (id: string, state = 'healthy', at = new Date().toISOString()) => ({ ...node(id), state, observed_at: at,
  last_seen_at: at, profile: 'gah', profiles: ['gah'], resource_pressure: { cpu_percent: 0, rss_bytes: null, disk_percent: 25 },
  active_claims: [{ work_id: '#946', scope: 'implement', pid: 123, claimed_at: at, age_seconds: 0 }], active_work: [],
  backend_configured: {}, backend_instances: [], availability: [], recent_ledger: null, event_cursor: null });

test('fleet lists unknown, stale and classified health; click checks health and WS invalidates cached data', async ({ mount, page }, testInfo) => {
  await connectSocket(page);
  let offline = false;
  let requests = 0;
  let healthChecks = 0;
  await page.route('**/api/registry/fleet/snapshot', (route) => {
    requests++;
    return route.fulfill({ json: { nodes: [node('one'), node('unknown'), node('stale'), node('bad')],
      observations: [offline ? { ...observation('one', 'unreachable'), active_claims: [], error: { kind: 'NETWORK', message: 'Worker offline' } } : observation('one'), observation('stale', 'healthy', '2020-01-01T00:00:00Z'),
        { ...observation('bad', 'auth_failed'), error: { kind: 'AUTH', message: 'Node returned HTTP 401' } }],
      leases: [{ node_id: 'one', work_id: '#946', profile: 'gah', renewed_at: new Date().toISOString(), expires_at: '2099-01-01T00:00:00Z' }] } });
  });
  await page.route('**/api/registry/nodes/one/health', (route) => {
    healthChecks++;
    return route.fulfill({ json: { node_id: 'one', status: 'healthy', state: 'healthy', timestamp: Date.now(), snapshot: observation('one') } });
  });
  const component = await mount(<WebSocketProvider><NodesPage /></WebSocketProvider>);
  await expect(component.getByRole('button', { name: 'Worker one', exact: true })).toBeVisible();
  await expect(component.getByText('Unknown — awaiting observation')).toBeVisible();
  await expect(component.getByText('Stale — last result: healthy')).toBeVisible();
  await expect(component.getByText('Unhealthy — auth failed')).toBeVisible();
  await expect(component.getByText('AUTH: Node returned HTTP 401')).toBeVisible();
  await expect(component.getByText('CPU: 0.0% · Memory: unknown · Disk: 25.0%')).toHaveCount(3);
  expect(requests).toBe(1); expect(healthChecks).toBe(0);
  await component.getByRole('button', { name: 'Worker one', exact: true }).click();
  const detail = component.getByRole('region', { name: 'Node detail' });
  await expect(detail.getByText(/Last manual check: healthy/)).toBeVisible();
  await expect(detail.getByText(/#946 · implement · PID 123/)).toBeVisible();
  await expect(detail.getByText(/gah \/ #946 · renewed/)).toBeVisible();
  expect(healthChecks).toBe(1);
  await expect.poll(() => requests).toBe(2);
  offline = true;
  await page.evaluate(() => {
    const socket = (window as unknown as { fleetSocket: WebSocket }).fleetSocket;
    socket.onmessage?.(new MessageEvent('message', { data: JSON.stringify({ type: 'fleet.changed' }) }));
  });
  await expect.poll(() => requests).toBe(3);
  await expect(detail.getByText('Local claims are unknown until the node returns a status snapshot.')).toBeVisible();
  await expect(detail.getByText(/#946 · implement · PID 123/)).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('nodes-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: testInfo.outputPath('nodes-mobile.png'), fullPage: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test('empty fleet offers installer and executable existing-worker registration command', async ({ mount, page }, testInfo) => {
  await connectSocket(page);
  await page.route('**/api/registry/fleet/snapshot', (route) => route.fulfill({ json: { nodes: [], observations: [], leases: [] } }));
  const component = await mount(<WebSocketProvider><NodesPage /></WebSocketProvider>);
  await expect(component.getByText(/No registered nodes/)).toBeVisible();
  await expect(component.getByRole('heading', { name: 'Add a Node' })).toBeVisible();
  await component.getByLabel('Central address', { exact: true }).fill('https://central.example.com');
  await component.getByLabel('Profiles (comma-separated)').fill('gah,tools');
  await component.getByRole('button', { name: 'Generate register-node command' }).click();
  const command = component.getByRole('textbox', { name: 'Register node command' });
  await expect(command).toHaveValue("npm run register-node --workspace=apps/server -- --central-url 'https://central.example.com' --self-url 'http://127.0.0.1:3773' --transport-mode 'trusted_lan' --secret-ref 'env:GAH_NODE_TOKEN' --profiles 'gah,tools'");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: testInfo.outputPath('register-mobile.png'), fullPage: true });
});

test('registry failure preserves the reason and supports retry', async ({ mount, page }) => {
  await connectSocket(page);
  await page.route('**/api/registry/fleet/snapshot', (route) => route.fulfill({ status: 503, json: { message: 'Registry storage unavailable' } }));
  const component = await mount(<WebSocketProvider><NodesPage /></WebSocketProvider>);
  await expect(component.getByRole('alert')).toContainText('Registry storage unavailable');
  await page.route('**/api/registry/fleet/snapshot', (route) => route.fulfill({ json: { nodes: [], observations: [], leases: [] } }));
  await component.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(component.getByText(/No registered nodes/)).toBeVisible();
  await expect(component.getByRole('alert')).toHaveCount(0);
});
