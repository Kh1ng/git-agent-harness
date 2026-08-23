import { defineConfig, devices } from '@playwright/test';

const port = process.env.GAH_WEB_TEST_PORT ?? '3000';
const baseURL = `http://localhost:${port}`;

/**
 * Minimal Playwright setup -- none existed before this pass. Assumes the
 * server (apps/server, port 3773) is already running and pointed at a real
 * `gah` config; this only drives the web frontend (port 3000). See
 * tests/e2e/smoke.spec.ts for what it checks: the five required viewport
 * classes, no horizontal overflow, and real content on each core page.
 */
export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: true,
  reporter: 'list',
  use: {
    baseURL,
    screenshot: 'only-on-failure'
  },
  webServer: {
    command: `npm run dev -- --port ${port}`,
    url: baseURL,
    reuseExistingServer: process.env.GAH_WEB_TEST_PORT === undefined,
    timeout: 30_000
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } }
  ]
});
