import { expect, test, type Locator, type Page } from '@playwright/test';

/**
 * Host-bridge invariant (docs/HOST_BRIDGE.md): the dashboard must never
 * emit a `target="_blank"` anchor. Embedded hosts discard new-window
 * requests, so such a link is dead on iOS and desktop regardless of what
 * it points at. Every external anchor must render as a plain anchor and
 * go through ExternalAnchor; this sweep pins the whole app rather than
 * the individual call sites. Pages are deep-linked directly because the
 * chat and git surfaces do not survive nav-button clicks under test.
 */

type Route = { path: string; marker: (page: Page) => Locator };

const ROUTES: Route[] = [
  { path: '/', marker: (page) => page.getByRole('heading', { name: 'Overview' }) },
  { path: '/?page=chat&profile=fixture&chat=default', marker: (page) => page.getByRole('button', { name: /Mock session/ }) },
  { path: '/?page=git&profile=fixture', marker: (page) => page.getByRole('heading', { name: 'Git' }) },
  { path: '/?page=nodes', marker: (page) => page.getByRole('heading', { name: 'Nodes' }) },
  { path: '/?page=work&profile=fixture', marker: (page) => page.getByRole('heading', { name: 'Factory' }) },
  { path: '/?page=telemetry&profile=fixture', marker: (page) => page.getByRole('heading', { name: 'Telemetry' }) },
  { path: '/?page=quota&profile=fixture', marker: (page) => page.getByRole('heading', { name: 'Quota' }) },
  { path: '/?page=activity&profile=fixture', marker: (page) => page.getByRole('heading', { name: 'Activity' }) },
  { path: '/?page=settings', marker: (page) => page.getByRole('heading', { name: 'Settings' }) },
];

test('no dashboard anchor ever opens a new window', async ({ page }) => {
  for (const route of ROUTES) {
    await page.goto(route.path, { waitUntil: 'domcontentloaded' });
    // The page must actually render before the sweep can mean anything.
    await expect(route.marker(page).first()).toBeVisible({ timeout: 10_000 });
    const targeted = await page.locator('a[target]').count();
    expect(targeted, `${route.path} must not render any new-window anchor`).toEqual(0);
  }
});
