import { expect, test } from '@playwright/test';

for (const scenario of [
  { pageName: 'Settings', section: 'General', paths: ['/api/manager-chat/settings', '/api/admin/update'] },
  { pageName: 'Settings', section: 'Skill bank', paths: ['/api/skills'] },
  { pageName: 'Settings', section: 'TDAI / memory', paths: ['/api/settings/gateway'] },
  { pageName: 'Git', section: '', paths: ['/api/git/status', '/api/git/log', '/api/git/prs'] },
  { pageName: 'Telemetry', section: '', paths: ['/api/usage/rollup'] }
]) {
  test(`${[scenario.pageName, scenario.section].filter(Boolean).join(" ")} recovers protected reads after the first token without navigation`, async ({ page }) => {
    const rejected = new Set<string>();
    const authenticated = new Set<string>();
    let authenticatedReads = 0;
    await page.route(url => scenario.paths.includes(url.pathname), async route => {
      const path = new URL(route.request().url()).pathname;
      if (route.request().headers().authorization !== 'Bearer settings-secret') {
        rejected.add(path);
        return route.fulfill({ status: 401, json: { message: 'Coordinator token required' } });
      }
      authenticated.add(path);
      authenticatedReads++;
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
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.locator('section').filter({ hasText: 'Which configured GAH repo' }).getByRole('combobox').selectOption('fixture');
    if (scenario.pageName === 'Settings') {
      const button = page.getByRole('button', { name: new RegExp(`^${scenario.section}`) });
      if (await button.getAttribute('aria-expanded') !== 'true') await button.click();
    } else {
      await page.getByRole('button', { name: scenario.pageName, exact: true }).click();
    }
    await expect.poll(() => [...rejected].sort()).toEqual([...scenario.paths].sort());
    if (scenario.pageName === 'Settings') {
      await page.getByLabel('Access token', { exact: true }).fill('settings-secret');
      await page.getByRole('button', { name: 'Save and reconnect' }).click();
    } else {
      // Keep the protected page mounted while exercising the shared credential event.
      await page.evaluate(() => {
        sessionStorage.setItem('gah.coordinatorToken', 'settings-secret');
        window.dispatchEvent(new Event('gah.coordinatorTokenChanged'));
      });
    }
    await expect.poll(() => [...authenticated].sort()).toEqual([...scenario.paths].sort());
    await expect(page.getByText(/Coordinator token required/)).toHaveCount(0);
    if (scenario.section === 'TDAI / memory') {
      const url = page.getByLabel('Gateway URL', { exact: true });
      await url.fill('https://unsaved.example.test');
      const before = authenticatedReads;
      if (!await page.getByRole('button', { name: 'Save and reconnect' }).isVisible()) {
        await page.getByText('Central access token', { exact: true }).click();
      }
      await page.getByRole('button', { name: 'Save and reconnect' }).click();
      await expect.poll(() => authenticatedReads).toBeGreaterThan(before);
      await expect(url).toHaveValue('https://unsaved.example.test');
    }
  });
}
