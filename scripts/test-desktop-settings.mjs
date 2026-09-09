// Exercise the bundled Settings UI without touching an installed app or worker.
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { createServer } from 'vite';

const server = await createServer({
  root: fileURLToPath(new URL('../apps/desktop', import.meta.url)),
  server: { host: '127.0.0.1', port: 0 },
});
let browser;
try {
  await server.listen();
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.addInitScript(() => {
    window.calls = [];
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      window.calls.push([command, args]);
      if (command === 'desktop_settings') return {
        central_url: 'http://central.test', wsl_distribution: '',
        presence: { dock: false, tray: true, launch_window: true },
      };
      if (command === 'save_presence') return args.presence;
      if (command === 'worker_status') return { running: false, note: 'Fixture only', tools: [] };
      if (command === 'set_worker_running') throw new Error('Worker unavailable');
    } };
  });
  await page.goto(server.resolvedUrls.local[0]);
  await page.getByRole('heading', { name: 'Settings · This computer' }).waitFor();
  await page.getByRole('button', { name: 'Back to Settings' }).click();
  assert.equal(await page.evaluate(() => window.calls.at(-1)[0]), 'open_central_settings');
  await page.getByLabel('Central node address').fill('http://another-central.test');
  await page.getByRole('button', { name: 'Save and connect' }).click();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), [
    'connect_dashboard', { settings: { central_url: 'http://another-central.test', wsl_distribution: '' } },
  ]);
  await page.getByLabel('Show tray icon').uncheck();
  assert(await page.getByLabel('Open a window on launch').isDisabled());
  assert(await page.getByLabel('Open a window on launch').isChecked());
  await page.getByRole('button', { name: 'Save app presence' }).click();
  await page.getByText('Saved. Icon changes apply now;', { exact: false }).waitFor();
  await page.getByRole('button', { name: 'Check worker' }).click();
  await page.getByText('Worker is stopped or has not been installed.').waitFor();
  await page.getByRole('button', { name: 'Start worker', exact: true }).click();
  await page.getByRole('alert').filter({ hasText: 'Worker unavailable' }).waitFor();
  assert(await page.getByRole('button', { name: 'Start worker', exact: true }).isEnabled());
  assert.equal(browser.contexts()[0].pages().length, 1);
  console.log('Desktop Settings: connection, back navigation, presence recovery, and worker failure checks passed.');
} finally {
  await browser?.close();
  await server.close();
}
