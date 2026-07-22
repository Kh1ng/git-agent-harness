import { expect, test, type Page } from '@playwright/test';

/**
 * Coverage for issue #750: every REST-backed page must show a visible
 * "last updated" readout, and a restored WebSocket connection must
 * re-trigger a fresh REST pull rather than leaving a panel stale until
 * its own timer happens to fire. Routes are fully mocked (call counts
 * tracked per endpoint) so this is deterministic regardless of whatever
 * real ledger data a shared dev backend happens to have at the moment.
 */

const callCounts: Record<string, number> = {};

function resetCounts() {
  for (const key of Object.keys(callCounts)) delete callCounts[key];
}

async function mockRestApi(page: Page) {
  await page.route('**/api/**', (route) => {
    const url = new URL(route.request().url());
    const key = url.pathname;
    callCounts[key] = (callCounts[key] ?? 0) + 1;

    switch (key) {
      case '/api/status':
        return route.fulfill({
          json: {
            profile: { display_name: 'GAH' },
            blockers: [],
            blocked_work_items: [],
            review_held_work_ids: [],
            merge_requests: [],
            recent_ledger: null,
            active_claims: [],
            issue_intake_rejections: [],
            available_tickets: [],
          },
        });
      case '/api/quota':
        return route.fulfill({ json: { candidates: [], usage: null } });
      case '/api/report':
        return route.fulfill({ json: { comparisons: [] } });
      case '/api/report/series':
        return route.fulfill({ json: { series: [], bucket: 'daily' } });
      case '/api/events':
        return route.fulfill({ json: [] });
      case '/api/profiles':
        return route.fulfill({ json: [] });
      case '/api/controller-activity':
        return route.fulfill({ json: [] });
      case '/api/loop/status':
        return route.fulfill({ json: { running: false } });
      default:
        // Anything else matching the broad glob below (notably Vite's own
        // dev-server module requests for files under src/api/) must pass
        // through untouched, or the app's JS bundle never loads.
        callCounts[key] -= 1;
        return route.continue();
    }
  });
}

/** Minimal server.welcome payload, enough for WebSocketContext to treat
 * the connection as ready without touching any of the optional fields
 * these tests don't exercise. */
const WELCOME_MESSAGE = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
};

test.describe('last-updated indicator', () => {
  test.beforeEach(async ({ page }) => {
    resetCounts();
    await mockRestApi(page);
  });

  for (const route of [
    { label: 'Overview', heading: 'Overview' },
    { label: 'Quota', heading: 'Quota' },
    { label: 'Telemetry', heading: 'Telemetry' },
    { label: 'Work', heading: 'Work' },
    { label: 'Events', heading: 'Events' },
  ]) {
    test(`${route.label} shows a live "Updated ... ago" readout once data loads`, async ({ page }) => {
      await page.goto('/');
      await page.getByRole('button', { name: route.label, exact: true }).click();
      await expect(page.getByRole('heading', { name: route.heading, exact: true })).toBeVisible();
      await expect(page.getByText(/^Updated (just now|\d+[smhd] ago)$/)).toBeVisible();
    });
  }
});

test.describe('WS reconnect re-triggers REST refetch', () => {
  test.beforeEach(async ({ page }) => {
    resetCounts();
    await mockRestApi(page);
  });

  test('Telemetry (a manual-only page before this fix) refetches its report data after the socket drops and reconnects', async ({ page }) => {
    let connectionCount = 0;

    await page.routeWebSocket('**/ws**', (ws) => {
      connectionCount += 1;
      const isFirstConnection = connectionCount === 1;
      ws.send(JSON.stringify(WELCOME_MESSAGE));
      if (isFirstConnection) {
        // Simulate a dropped connection shortly after connecting -- the
        // client's own reconnect timer (WebSocketContext.tsx) picks this
        // up and re-opens a new socket, which this route re-intercepts.
        setTimeout(() => ws.close(), 300);
      }
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'Telemetry', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Telemetry', exact: true })).toBeVisible();

    await expect.poll(() => callCounts['/api/report'] ?? 0).toBeGreaterThanOrEqual(1);
    const initialReportCalls = callCounts['/api/report'] ?? 0;
    const initialSeriesCalls = callCounts['/api/report/series'] ?? 0;

    // Wait out the drop + the client's reconnect delay, then confirm a
    // second connection actually happened and the page pulled fresh data
    // in response, without the user touching the manual refresh button.
    await expect.poll(() => connectionCount, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
    await expect.poll(() => callCounts['/api/report'] ?? 0, { timeout: 15_000 }).toBeGreaterThan(initialReportCalls);
    await expect.poll(() => callCounts['/api/report/series'] ?? 0).toBeGreaterThan(initialSeriesCalls);
  });
});
