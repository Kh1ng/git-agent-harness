import { expect, test } from '@playwright/test';

test('saving the first valid token restores a mounted REST panel without navigation', async ({ page }) => {
  let rejectedReads = 0;
  let authenticatedReads = 0;
  await page.route(url => url.pathname === '/api/status', async route => {
    if (route.request().headers().authorization !== 'Bearer dashboard-secret') {
      rejectedReads++;
      return route.fulfill({ status: 401, json: { message: 'Coordinator token required' } });
    }
    authenticatedReads++;
    return route.continue();
  });
  // An actual rejected upgrade never fires onopen. Closing a mocked socket
  // after upgrade would incorrectly mark the first connection successful.
  await page.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    window.WebSocket = class extends NativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        const target = new URL(url);
        if (!sessionStorage.getItem('gah.coordinatorToken') && target.pathname === '/ws') target.port = '0';
        super(target, protocols);
      }
    };
  });
  await page.goto('/');
  // Pin the profile before authentication so welcome cannot recover the panel
  // merely by changing its profile key. Only the credential refresh can recover it.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.locator('section').filter({ hasText: 'Which configured GAH repo' }).getByRole('combobox').selectOption('fixture');
  await page.getByRole('button', { name: 'Overview', exact: true }).click();
  await expect.poll(() => rejectedReads).toBeGreaterThan(0);
  await expect(page.getByRole('alert')).toContainText('Coordinator token required');
  await page.getByLabel('Access token', { exact: true }).fill('dashboard-secret');
  await page.getByRole('button', { name: 'Save and reconnect' }).click();
  await expect.poll(() => authenticatedReads).toBeGreaterThan(0);
  await expect(page.getByRole('alert').filter({ hasText: 'Coordinator token required' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
});

test('the first token restores mounted Chat projects, provider choices, git, and open storage', async ({ page, request }) => {
  await request.post('http://127.0.0.1:3774/api/mock/scenario', { data: { name: 'normal' } });
  let protectReads = false;
  const rejected = new Set<string>();
  const authenticated = new Set<string>();
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (protectReads && route.request().headers().authorization !== 'Bearer chat-secret') {
      rejected.add(path);
      return route.fulfill({ status: 401, json: { message: 'Coordinator token required' } });
    }
    if (protectReads) authenticated.add(path);
    return route.continue();
  });
  await page.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    window.WebSocket = class extends NativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        const target = new URL(url);
        if (!sessionStorage.getItem('gah.coordinatorToken') && target.pathname === '/ws') target.port = '0';
        super(target, protocols);
      }
    };
  });
  await page.goto('/');
  // Pin through the UI before protecting reads. Welcome cannot repair Chat by
  // changing its profile; every Chat mount request below must fail until login.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.locator('section').filter({ hasText: 'Which configured GAH repo' }).getByRole('combobox').selectOption('fixture');
  protectReads = true;
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  const initialPaths = ['/api/profiles', '/api/manager-chat/settings', '/api/manager-chat/commands', '/api/manager-chat/models', '/api/git/status', '/api/manager-chat/issues', '/api/manager-chat/prs', '/api/manager-chat/storage'];
  await expect.poll(() => initialPaths.every(path => rejected.has(path))).toBe(true);
  await expect(page.getByRole('navigation', { name: 'Projects', exact: true }).getByRole('button')).toHaveCount(0);
  await page.getByLabel('Access token', { exact: true }).fill('chat-secret');
  await page.getByRole('button', { name: 'Save and reconnect' }).click();
  await expect(page.getByRole('navigation', { name: 'Projects', exact: true }).getByRole('button', { name: /Fixture/ })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex');
  await expect(page.getByText('feat/mock-control-plane-1087', { exact: true })).toBeVisible();
  await expect(page.getByText(/projected reclaim · idle after/)).toBeVisible();
  await expect(page.getByLabel('Project skills', { exact: true })).toBeVisible();
  await expect.poll(() => [...initialPaths, '/api/skills/bindings'].every(path => authenticated.has(path))).toBe(true);
  await expect(page.getByText('Coordinator token required', { exact: true })).toHaveCount(0);
  // The session backend has its own model loader, independent of the default composer.
  await page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ }).click();
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex');
});
