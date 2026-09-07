import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import type { QuotaSnapshot } from '@git-agent-harness/contracts';

const fixture: QuotaSnapshot = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8'));

test('Settings and Quota share candidate eligibility, timestamps, refresh failures, and SCM refresh', async ({ page }) => {
  const snapshot = structuredClone(fixture);
  snapshot.candidates = [{ ...snapshot.candidates[0], backend: 'agy', model: 'test-model', quota_pool: 'weekly', eligible_now: false, reason: 'Account quota exhausted', observed_at: '2026-08-01T10:00:00Z' }];
  let fail = false;
  let scmRefreshes = 0;
  await page.route('**/api/quota?*', (route) => fail ? route.fulfill({ status: 503, json: { message: 'Offline test' } }) : route.fulfill({ json: snapshot }));
  await page.routeWebSocket('**/ws', (ws) => {
    const server = ws.connectToServer();
    server.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'server.welcome') {
        message.serverProviderCatalog.providers = [
          { instanceId: 'github-test', providerKind: 'github', name: 'GitHub test' },
          { instanceId: 'agy-test', providerKind: 'agy', name: 'Boot-time AGY' }
        ];
        message.providers = { 'github-test': { type: 'authenticated', version: 'test', userId: 'scm-user' }, 'agy-test': { type: 'available', version: 'boot' } };
      }
      ws.send(JSON.stringify(message));
    });
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'provider.refresh') scmRefreshes++;
      server.send(raw);
    });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const backends = page.getByRole('region', { name: /Agent backends/ });
  await expect(backends.getByText('Unavailable', { exact: true })).toBeVisible();
  await expect(backends).toContainText('Account quota exhausted');
  await expect(backends.locator('time[datetime="2026-08-01T10:00:00Z"]')).toBeVisible();
  await expect(backends).not.toContainText('Boot-time AGY');
  await page.getByRole('button', { name: /GitHub test/ }).click();
  await expect.poll(() => scmRefreshes).toBe(1);
  await page.getByRole('button', { name: 'Quota', exact: true }).click();
  await expect(page.getByText('Reason: Account quota exhausted')).toBeVisible();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  snapshot.candidates[0].eligible_now = true;
  snapshot.candidates[0].reason = null;
  await page.getByRole('button', { name: 'Refresh', exact: true }).first().click();
  await expect(backends.getByText('Eligible', { exact: true })).toBeVisible();
  fail = true;
  await page.getByRole('button', { name: 'Refresh', exact: true }).first().click();
  await expect(backends.getByRole('alert')).toContainText('Showing the last snapshot');
  await expect(backends.getByText('Eligible', { exact: true })).toBeVisible();
});

test('a late quota response cannot replace availability after a profile switch', async ({ page }) => {
  let releaseFirst!: () => void;
  const firstReleased = new Promise<void>((resolve) => { releaseFirst = resolve; });
  let firstRequested = false;
  await page.route('**/api/profiles', async (route) => {
    const profiles = await (await route.fetch()).json();
    await route.fulfill({ json: [...profiles, { ...profiles[0], name: 'second', display_name: 'Second' }] });
  });
  await page.route('**/api/quota?*', async (route) => {
    const profile = new URL(route.request().url()).searchParams.get('profile');
    if (profile === 'fixture') { firstRequested = true; await firstReleased; }
    const snapshot = structuredClone(fixture);
    snapshot.profile.profile = profile ?? 'fixture';
    snapshot.candidates = [{ ...snapshot.candidates[0], backend: profile === 'second' ? 'second-backend' : 'old-backend' }];
    await route.fulfill({ json: snapshot });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect.poll(() => firstRequested).toBe(true);
  const backends = page.getByRole('region', { name: /Agent backends/ });
  await page.locator('section').filter({ hasText: 'Which configured GAH repo' }).getByRole('combobox').selectOption('second');
  await expect(backends).toContainText('second-backend');
  const lateResponse = page.waitForResponse((response) => response.url().includes('/api/quota?') && new URL(response.url()).searchParams.get('profile') === 'fixture');
  releaseFirst();
  await (await lateResponse).finished();
  await expect(backends).toContainText('second-backend');
  await expect(backends).not.toContainText('old-backend');
  await page.getByRole('button', { name: 'Quota', exact: true }).click();
  await expect(page.getByText(/^second-backend \//)).toBeVisible();
  await expect(page.getByText(/^old-backend \//)).toHaveCount(0);
});
