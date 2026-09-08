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
