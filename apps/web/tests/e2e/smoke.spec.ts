import { test, expect } from '@playwright/test';

/**
 * Smoke coverage for the frontend productization pass. Not exhaustive --
 * it exists to catch regressions in the two things that were manually
 * verified during this pass and are easy to silently break later:
 * (1) every core route renders without horizontal overflow at each
 *     required viewport class, and (2) navigation between routes actually
 *     works. It intentionally does not assert against live gah data
 *     (CI/dev environments won't have a populated ledger) -- assertions
 *     are on structure (headings, nav, empty states), not values.
 */

const VIEWPORTS = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'wide-desktop', width: 1728, height: 1117 },
  { name: 'tablet', width: 768, height: 1024 },
  { name: 'mobile', width: 390, height: 844 },
  { name: 'small-mobile', width: 360, height: 800 }
];

const ROUTES: { label: string; heading: string }[] = [
  { label: 'Overview', heading: 'Overview' },
  { label: 'Manager Chat', heading: 'Manager Chat' },
  { label: 'Work', heading: 'Work' },
  { label: 'Telemetry', heading: 'Telemetry' },
  { label: 'Quota', heading: 'Quota' },
  { label: 'Events', heading: 'Events' },
  { label: 'Settings', heading: 'Settings' }
];

async function navigateTo(page: import('@playwright/test').Page, label: string, isMobile: boolean) {
  if (isMobile) {
    const menuButton = page.getByRole('button', { name: 'Open navigation menu' });
    if (await menuButton.isVisible()) {
      await menuButton.click();
    }
  }
  await page.getByRole('button', { name: label, exact: true }).click();
}

for (const viewport of VIEWPORTS) {
  test.describe(`${viewport.name} (${viewport.width}x${viewport.height})`, () => {
    test.use({ viewport: { width: viewport.width, height: viewport.height } });

    test('every route renders with no horizontal overflow', async ({ page }) => {
      await page.goto('/');
      const isMobile = viewport.width < 1024;

      for (const route of ROUTES) {
        await navigateTo(page, route.label, isMobile);
        await expect(page.getByRole('heading', { name: route.heading, exact: true })).toBeVisible();

        const { scrollWidth, clientWidth } = await page.evaluate(() => ({
          scrollWidth: document.documentElement.scrollWidth,
          clientWidth: document.documentElement.clientWidth
        }));
        expect(scrollWidth, `${route.label} should not overflow horizontally at ${viewport.name}`).toBeLessThanOrEqual(
          clientWidth + 1 // 1px tolerance for scrollbar rounding
        );
      }
    });
  });
}

test.describe('desktop content', () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test('sidebar navigation is present with every primary section', async ({ page }) => {
    await page.goto('/');
    const nav = page.getByRole('navigation', { name: 'Primary' });
    for (const route of ROUTES) {
      await expect(nav.getByRole('button', { name: route.label, exact: true })).toBeVisible();
    }
  });

  test('manager chat switches backend in place and shows project node context', async ({ page }) => {
    let backend = 'hermes';
    await page.route('**/api/profiles', (route) => route.fulfill({
      json: [{
        name: 'gah',
        display_name: 'Git Agent Harness',
        provider: 'github',
        repo: 'Kh1ng/git-agent-harness',
        local_path: '/workspace/git-agent-harness'
      }]
    }));
    await page.route('**/api/manager-chat/settings', async (route) => {
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON() as { profileOverrides?: Record<string, string> };
        backend = body.profileOverrides?.gah ?? backend;
        await route.fulfill({ json: { success: true } });
        return;
      }
      await route.fulfill({
        json: {
          defaultBackend: 'hermes',
          profileOverrides: backend === 'hermes' ? {} : { gah: backend },
          availableBackends: [
            { id: 'hermes', displayName: 'Hermes', implemented: true },
            { id: 'codex', displayName: 'Codex', implemented: true }
          ]
        }
      });
    });
    await page.route('**/api/manager-chat/commands**', (route) => route.fulfill({ json: { commands: [] } }));
    await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: { models: [], currentModelId: null } }));
    await page.route('**/api/registry/fleet**', (route) => route.fulfill({
      json: [{ node_id: 'node-1', display_name: 'Build Mac', state: 'healthy' }]
    }));

    await page.goto('/');
    await navigateTo(page, 'Manager Chat', false);
    await expect(page.getByText('Shared project context')).toBeVisible();
    await expect(page.getByText('Build Mac')).toBeVisible();
    const backendPicker = page.getByLabel('Backend');
    await expect(backendPicker).toHaveValue('hermes');
    await backendPicker.selectOption('codex');
    await expect(backendPicker).toHaveValue('codex');
    await expect(page.getByRole('heading', { name: 'Manager Chat' })).toBeVisible();
  });

  test('theme toggle switches data-theme attribute', async ({ page }) => {
    await page.goto('/');
    await navigateTo(page, 'Settings', false);
    await page.getByRole('button', { name: 'Light' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await page.getByRole('button', { name: 'Dark' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  });

  test('quota page never shows a bare 0% for an unknown observation', async ({ page }) => {
    await page.goto('/');
    await navigateTo(page, 'Quota', false);
    // The page's own title always renders (even mid-load or on a total
    // data-fetch failure -- see PageHeader placement in QuotaPage.tsx), and
    // whatever the data state, the literal string "0%" must never appear:
    // an unknown/errored observation renders as "No observation" text or
    // an explicit error card, never a silently-zeroed progress indicator.
    await expect(page.getByRole('heading', { name: 'Quota', exact: true })).toBeVisible();
    await expect(page.getByText('0%', { exact: true })).toHaveCount(0);
  });
});
