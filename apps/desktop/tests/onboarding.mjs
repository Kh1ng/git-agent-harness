// Frontend integration evidence with a mocked Tauri host. This does not validate native installation.
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { readFileSync, mkdirSync } from 'node:fs';
import { resolve, extname } from 'node:path';
const dist = new URL('../dist', import.meta.url).pathname;
const evidence = process.env.GAH_GUI_EVIDENCE_DIR;
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  await page.route('http://gah.test/**', route => {
    const pathname = new URL(route.request().url()).pathname;
    const file = resolve(dist, pathname === '/' ? 'index.html' : pathname.slice(1));
    assert.ok(file.startsWith(dist + '/') || file === dist);
    route.fulfill({ body: readFileSync(file), contentType: extname(file) === '.js' ? 'text/javascript' : 'text/html' });
  });
  await page.addInitScript(() => {
    const callbacks = {};
    const listeners = {};
    window.calls = [];
    window.attempt = 0;
    window.__TAURI_INTERNALS__ = {
      transformCallback(fn) { const id = Object.keys(callbacks).length; callbacks[id] = fn; return id; },
      async invoke(command, args) {
        window.calls.push({ command, args });
        if (command === 'plugin:event|listen') { listeners[args.event] = args.handler; return 1; }
        if (command === 'repository_tools') return [{ program: 'gh', installed: true }, { program: 'glab', installed: false }];
        if (command === 'desktop_settings') return { central_url: '', wsl_distribution: '', presence: { dock: false, tray: true, launch_window: true } };
        if (command === 'owner_credential_status') return { saved: false };
        if (command === 'node_role_status') return { role: 'central', running: false, supported: false };
        if (command === 'credential_list') return [];
        if (command === 'setup_check') return { installed: true, report: { ready: false, requirements: [{ id: 'provider_login', label: 'Repository login', why: 'Authenticate before setup', optional: false, status: { state: 'not_logged_in' }, action: { command: 'gh auth login', sudo: false } }] } };
        if (command === 'onboarding_run') {
          const failed = ++window.attempt === 1;
          callbacks[listeners['gah:onboarding-progress']]?.({ event: 'gah:onboarding-progress', payload: failed ? 'Native permission prompt denied; retry available.' : 'Local dashboard ready.' });
          if (failed) throw new Error('Permission was denied');
          return 'http://127.0.0.1:3773';
        }
        if (command === 'connect_dashboard') return undefined;
        if (command === 'onboarding_login') return undefined;
        return undefined;
      },
    };
  });
  await page.goto('http://gah.test/');
  await page.locator('#setup-state').filter({ hasText: 'required' }).waitFor();
  await page.selectOption('#setup-agent', 'codex');
  await page.locator('#setup-standalone').click();
  await page.locator('#setup-state').filter({ hasText: 'Permission was denied' }).waitFor();
  assert.equal(await page.inputValue('#setup-agent'), 'codex');
  assert.match(await page.locator('#setup-progress').textContent(), /permission prompt denied/);
  if (evidence) {
    mkdirSync(evidence, { recursive: true });
    await page.locator('#setup-section').screenshot({ path: resolve(evidence, 'onboarding-failure.png') });
  }
  await page.locator('#setup-standalone').click();
  await page.locator('#setup-state').filter({ hasText: 'Setup complete' }).waitFor();
  if (evidence) await page.locator('#setup-section').screenshot({ path: resolve(evidence, 'onboarding-retry.png') });
  const calls = await page.evaluate(() => window.calls);
  const installs = calls.filter(call => call.command === 'onboarding_run');
  assert.equal(installs.length, 2);
  assert.deepEqual(installs[0].args.choices, { role: 'standalone', agent: 'codex', provider: 'github', memory: 'off', gateway_url: '' });
  assert.deepEqual(installs[0].args.choices, installs[1].args.choices);
  assert.ok(calls.some(call => call.command === 'connect_dashboard'));
  assert.ok(!calls.some(call => call.command === 'open_setup_terminal'));
  await page.fill('#setup-repository-token', 'synthetic-test-token');
  await page.locator('#setup-repository-login').click();
  await page.waitForFunction(() => document.querySelector('#setup-repository-token').value === '');
  assert.ok(!(await page.locator('#setup-progress').textContent()).includes('synthetic-test-token'));
  const loginCount = (await page.evaluate(() => window.calls)).filter(call => call.command === 'onboarding_login').length;
  await page.selectOption('#repository-provider', 'glab');
  await page.waitForFunction(() => document.querySelector('#setup-repository-login').disabled);
  assert.equal(await page.locator('#setup-repository-login').isDisabled(), true);
  assert.equal((await page.evaluate(() => window.calls)).filter(call => call.command === 'onboarding_login').length, loginCount);
  assert.deepEqual(errors, []);
  console.log('GUI failure/retry, explicit choices, local completion, missing-package login blocking and terminal-free workflow passed (mocked host).');
} finally { await browser.close(); }
