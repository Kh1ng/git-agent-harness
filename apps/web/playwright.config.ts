import { defineConfig, devices } from '@playwright/test';

const port = process.env.GAH_WEB_TEST_PORT ?? '3000';
const baseURL = `http://localhost:${port}`;

/**
 * Hermetic Playwright setup (issue #636). Two webServers:
 *
 * 1. The stateful mock control plane on :3774. It replays the committed gah
 *    response fixtures for dashboard REST data and owns manager-chat REST/WS
 *    state in memory. It imports no provider/worktree/production-state code.
 *    Port 3774 (not 3773) so a real control plane / t3 server on 3773 never
 *    leaks into the test.
 * 2. The Vite dev server on GAH_WEB_TEST_PORT (default 3000), whose /api +
 *    /ws proxies target :3774.
 *
 * This is what makes `npm run e2e --workspace=apps/web` pass on a machine
 * with no gah binary and no gah config.
 */
export default defineConfig({
  testDir: './tests/e2e',
  // All specs share one intentionally global, stateful mock control plane.
  // Keep scenario selection/reset deterministic across files as well as CI.
  workers: 1,
  fullyParallel: false,
  // One retry on CI only: the shared mock+vite servers run on a 2-core
  // runner and Settings is the heaviest page; a single retry absorbs the
  // residual load-induced flake without masking real regressions.
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL,
    screenshot: 'only-on-failure'
  },
  webServer: [
    {
      command: 'npm run dev:mock -- --host 127.0.0.1 --port 3774 --scenario normal',
      cwd: '../server',
      url: 'http://127.0.0.1:3774/health',
      reuseExistingServer: false,
      timeout: 60_000
    },
    {
      command: `VITE_PROXY_TARGET=http://localhost:3774 VITE_WS_PROXY_TARGET=ws://localhost:3774 npm run dev -- --port ${port}`,
      url: baseURL,
      reuseExistingServer: false,
      timeout: 30_000
    }
  ],
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } }
  ]
});
