import { test, expect } from '@playwright/test';
import { openUsage } from './helpers/navigation.js';

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

/** A navbar entry, and for a grouped page the tab that shows it. */
const ROUTES: { label: string; tab?: string; usage?: 'Telemetry' | 'Quota'; heading: string }[] = [
  { label: 'Overview', heading: 'Overview' },
  { label: 'Fleet', heading: 'Fleet' },
  { label: 'Factory', heading: 'Factory' },
  { label: 'Usage', usage: 'Telemetry', heading: 'Telemetry' },
  { label: 'Usage', usage: 'Quota', heading: 'Quota management' },
  { label: 'Projects', heading: 'Git' },
  { label: 'Projects', tab: 'Planning', heading: 'Planning' },
  { label: 'Activity', heading: 'Activity' },
  { label: 'Settings', heading: 'Settings' }
];

test.beforeEach(async ({ page }) => {
  await page.route('**/api/registry/fleet/snapshot', (route) => route.fulfill({
    json: { nodes: [], observations: [], leases: [] }
  }));
});

async function navigateTo(page: import('@playwright/test').Page, label: string, isMobile: boolean, tab?: string) {
  if (isMobile) {
    const menuButton = page.getByRole('button', { name: 'Open navigation menu' });
    if (await menuButton.isVisible()) {
      await menuButton.click();
    }
  }
  await page.getByRole('button', { name: label, exact: true }).click();
  if (tab) await page.getByRole('navigation', { name: 'Page tabs' }).getByRole('button', { name: tab, exact: true }).click();
}

for (const viewport of VIEWPORTS) {
  test.describe(`${viewport.name} (${viewport.width}x${viewport.height})`, () => {
    test.use({ viewport: { width: viewport.width, height: viewport.height } });

    test('every route renders with no horizontal overflow', async ({ page }) => {
      await page.goto('/');
      const isMobile = viewport.width < 1024;

      for (const route of ROUTES) {
        if (route.usage) await openUsage(page, route.usage);
        else await navigateTo(page, route.label, isMobile, route.tab);
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

  test('the top navbar and the sidebar strip hold all core sections', async ({ page }) => {
    await page.goto('/');
    const nav = page.getByRole('navigation', { name: 'Primary' });
    const sidebar = page.getByRole('navigation', { name: 'Sidebar' });
    for (const route of ROUTES) {
      const owner = route.label === 'Activity' || route.label === 'Settings' ? sidebar : nav;
      await expect(owner.getByRole('button', { name: route.label, exact: true })).toBeVisible();
    }
  });

  test('a sidebar icon opens its view beside the main panel and closes it again', async ({ page }) => {
    await page.goto('/?page=git');
    const settings = page.getByRole('navigation', { name: 'Sidebar' }).getByRole('button', { name: 'Settings', exact: true });
    const panel = page.getByRole('complementary', { name: 'Settings' });
    await settings.click();
    await expect(settings).toHaveAttribute('aria-expanded', 'true');
    await expect(panel.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
    await expect(page.getByRole('main')).toBeVisible();
    const share = await panel.evaluate((element) => element.getBoundingClientRect().width / window.innerWidth);
    expect(share).toBeGreaterThan(0.2);
    expect(share).toBeLessThan(0.35);
    expect(new URL(page.url()).searchParams.get('side')).toBe('settings');
    await openUsage(page, 'Quota');
    await expect(panel).toBeVisible();
    await settings.click();
    await expect(panel).toHaveCount(0);
    expect(new URL(page.url()).searchParams.get('side')).toBeNull();
  });

  test('Projects shows its pages as tabs; Usage opens Telemetry and Quota from a dropdown', async ({ page }) => {
    await page.goto('/');
    const nav = page.getByRole('navigation', { name: 'Primary' });
    await expect(page.getByRole('navigation', { name: 'Page tabs' })).toHaveCount(0);
    await nav.getByRole('button', { name: 'Usage', exact: true }).click();
    const menu = page.getByRole('menu', { name: 'Usage' });
    await expect(menu.getByRole('menuitem')).toHaveText(['Telemetry', 'Quota']);
    await menu.getByRole('menuitem', { name: 'Quota' }).click();
    await expect(menu).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Quota management', exact: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('page')).toBe('quota');
    // A dropdown group has no tabs above the page; its entry is the current one.
    await expect(page.getByRole('navigation', { name: 'Page tabs' })).toHaveCount(0);
    await expect(nav.getByRole('button', { name: 'Usage', exact: true })).toHaveAttribute('aria-current', 'page');
    await nav.getByRole('button', { name: 'Usage', exact: true }).click();
    await expect(menu.getByRole('menuitem', { name: 'Quota' })).toHaveAttribute('aria-current', 'page');
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
    // Projects still shows Git and Planning as tabs.
    await nav.getByRole('button', { name: 'Projects', exact: true }).click();
    await expect(page.getByRole('navigation', { name: 'Page tabs' }).getByRole('button')).toHaveText(['Git', 'Planning']);
    // Fleet is always listed: it is where a standalone install adds its first node.
    await nav.getByRole('button', { name: 'Fleet', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Fleet', exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Node readiness' })).toBeVisible();
  });

  test('theme picker switches data-theme and keeps it across reloads', async ({ page }) => {
    await page.goto('/');
    await navigateTo(page, 'Settings', false);
    await page.getByRole('button', { name: 'Light' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await page.getByRole('button', { name: 'Gruvbox' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'gruvbox');
    await page.reload();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'gruvbox');
    await page.getByRole('button', { name: 'Dark' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  });

  test('quota page never shows a bare 0% for an unknown observation', async ({ page }) => {
    await page.goto('/');
    await openUsage(page, 'Quota');
    // The page's own title always renders (even mid-load or on a total
    // data-fetch failure -- see PageHeader placement in QuotaPage.tsx), and
    // whatever the data state, the literal string "0%" must never appear:
    // an unknown/errored observation renders as "No observation" text or
    // an explicit error card, never a silently-zeroed progress indicator.
    await expect(page.getByRole('heading', { name: 'Quota management', exact: true })).toBeVisible();
    await expect(page.getByText('0%', { exact: true })).toHaveCount(0);
  });
});

test('choosing the current group closes a sidebar view that covers the page', async ({ page }) => {
  // Below 1280px an open sidebar view takes the content area.
  await page.setViewportSize({ width: 1100, height: 800 });
  await page.goto('/?page=overview&profile=fixture&side=settings');
  await expect(page).toHaveURL(/[?&]side=settings/);
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: 'Overview', exact: true }).click();
  await expect(page).not.toHaveURL(/[?&]side=/);
  await expect(page).toHaveURL(/[?&]page=overview/);
});
