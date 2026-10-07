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
    window.repositoryInstalled = false;
    window.factoryEnabled = false;
    window.setupInstalled = false;
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      window.calls.push([command, args]);
      if (command === 'repository_tools') return [{ program: 'gh', installed: window.repositoryInstalled }, { program: 'glab', installed: false }];
      if (command === 'desktop_settings') return {
        central_url: 'http://central.test', wsl_distribution: '',
        presence: { dock: false, tray: true, launch_window: true },
      };
      if (command === 'open_setup_terminal') { window.setupInstalled = true; window.factoryEnabled = args.factoryEnabled ?? false; return 'http://127.0.0.1:3773'; }
      if (command === 'setup_check' && !window.setupInstalled) return { installed: false, report: null, terminal: true };
      if (command === 'setup_check') return { installed: true, report: { ready: true, application_ready: true, factory_enabled: window.factoryEnabled, factory_ready: window.factoryEnabled, requirements: [] }, terminal: true };
      if (command === 'set_factory_enabled') { window.factoryEnabled = args.enabled; return; }
      if (command === 'node_role_status') return { role: 'worker', running: false, supported: true };
      if (command === 'set_node_role') return { role: args.role, running: true, supported: true };
      if (command === 'save_presence') return args.presence;
      if (command === 'worker_status') return { running: false, note: 'Fixture only', tools: [] };
      if (command === 'set_worker_running') throw new Error('Worker unavailable');
    } };
  });
  await page.goto(server.resolvedUrls.local[0]);
  await page.getByRole('heading', { name: 'Settings · This computer' }).waitFor();
  await page.getByText('Install it before signing in, then select Check installation.', { exact: false }).waitFor();
  assert.equal(await page.getByRole('link', { name: 'Install GitHub CLI' }).getAttribute('href'), 'https://cli.github.com/');
  await page.getByRole('link', { name: 'Install GitHub CLI' }).click();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['open_external_url', { url: 'https://cli.github.com/' }]);
  await page.evaluate(() => { window.repositoryInstalled = true; });
  await page.getByRole('button', { name: 'Check installation', exact: true }).click();
  await page.getByText('GitHub CLI (gh) is installed.', { exact: false }).waitFor();
  assert(await page.getByRole('link', { name: 'Install GitHub CLI' }).isHidden());
  await page.getByLabel('Repository host').selectOption('glab');
  await page.getByText('GitLab CLI (glab) is required', { exact: false }).waitFor();
  assert.equal(await page.getByRole('link', { name: 'Install GitLab CLI' }).getAttribute('href'), 'https://gitlab.com/gitlab-org/cli#installation');
  assert.equal(await page.getByLabel('Enable factory automation').isChecked(), false);
  // The checkbox was never loaded from the host, so setup must leave the choice to the installer.
  await page.getByRole('button', { name: 'Set up standalone' }).click();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['open_setup_terminal', { standalone: true }]);
  await page.evaluate(() => { window.setupInstalled = false; });
  await page.getByLabel('Enable factory automation').setChecked(true);
  await page.getByLabel('Enable factory automation').setChecked(false);
  await page.getByRole('button', { name: 'Set up standalone' }).click();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['open_setup_terminal', { standalone: true, factoryEnabled: false }]);
  await page.getByRole('button', { name: 'Check again', exact: true }).click();
  await page.getByText('Factory module disabled · local application remains available.').waitFor();
  assert.equal(await page.getByLabel('Enable factory automation').isChecked(), false);
  for (const enabled of [true, false, true, false]) {
    await page.getByLabel('Enable factory automation').setChecked(enabled);
    await page.getByRole('button', { name: 'Save factory module' }).click();
    await page.getByText(enabled ? 'Factory module enabled · prerequisites ready.' : 'Factory module disabled · local application remains available.', { exact: false }).waitFor();
    assert.equal(await page.evaluate(() => window.factoryEnabled), enabled);
  }
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
  await page.getByText('Worker mode · stopped.', { exact: false }).waitFor();
  await page.getByRole('button', { name: 'Use as central' }).click();
  await page.getByText('Central mode · running.', { exact: false }).waitFor();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['set_node_role', { role: 'central' }]);
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
