import { defineConfig, devices } from '@playwright/test';

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
    baseURL: 'http://localhost:3000',
    screenshot: 'only-on-failure'
  },
  webServer: {
    command: 'npm run dev',
    url: 'http://localhost:3000',
    reuseExistingServer: true,
    timeout: 30_000
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } }
  ]
});
