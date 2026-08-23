import { defineConfig, devices } from '@playwright/test';

/**
 * Hermetic Playwright setup (issue #636). Two webServers:
 *
 * 1. A fixture-backed apps/server on :3774 -- `GAH_BINARY` points at the
 *    fake `gah` fixture (apps/server/tests/fixtures/gah), so every REST/WS
 *    call the web app makes is answered with deterministic recorded output
 *    instead of a real gah binary + operator config. Runs against committed
 *    fixture identity/registry files so it never writes into the repo.
 *    Port 3774 (not 3773) so a real control plane / t3 server on 3773 never
 *    leaks into the test.
 * 2. The vite dev server on :3000, whose /api + /ws proxies target :3774.
 *
 * This is what makes `npm run e2e --workspace=apps/web` pass on a machine
 * with no gah binary and no gah config.
 */
export default defineConfig({
  testDir: './tests/e2e',
  // Cap workers + disable intra-file parallelism: the e2e suite shares one
  // fixture-backed apps/server and one vite dev server, and 6 parallel
  // browsers running all tests in a file simultaneously (fullyParallel)
  // saturates them on a 2-core CI runner -- the Settings page's serial
  // fetch chain intermittently failed to render its heading under that load.
  // Tests within a file now run sequentially; different files still
  // parallelize up to the worker cap.
  workers: process.env.CI ? 4 : undefined,
  fullyParallel: false,
  // One retry on CI only: the shared fixture+vite servers run on a 2-core
  // runner and Settings is the heaviest page; a single retry absorbs the
  // residual load-induced flake without masking real regressions.
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL: 'http://localhost:3000',
    screenshot: 'only-on-failure'
  },
  webServer: [
    {
      command:
        'PATH=./tests/fixtures/e2e/bin:$PATH ' +
        'GAH_BINARY=./tests/fixtures/gah/gah ' +
        'GAH_COORDINATOR_IDENTITY_PATH=./tests/fixtures/e2e/identity.json ' +
        'GAH_REGISTRY_CONFIG_PATH=./tests/fixtures/e2e/registry.json ' +
        'HOST=127.0.0.1 PORT=3774 npx tsx src/bin.ts',
      cwd: '../server',
      url: 'http://127.0.0.1:3774/health',
      reuseExistingServer: false,
      timeout: 60_000
    },
    {
      command: 'VITE_PROXY_TARGET=http://localhost:3774 VITE_WS_PROXY_TARGET=ws://localhost:3774 npm run dev',
      url: 'http://localhost:3000',
      reuseExistingServer: false,
      timeout: 30_000
    }
  ],
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } }
  ]
});
