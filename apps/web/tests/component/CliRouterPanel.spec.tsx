import { test, expect } from '@playwright/experimental-ct-react';
import type { Page } from '@playwright/test';
import React from 'react';
import { CliRouterPanel } from '../../src/components/CliRouterPanel.js';
import type { CliRouterSnapshot } from '../../src/api/client.js';

function makeSnapshot(overrides: Partial<CliRouterSnapshot> = {}): CliRouterSnapshot {
  return {
    settings: { url: 'https://router.example.com', hasApiKey: true, hasManagementKey: true },
    status: 'connected',
    strategy: 'round-robin',
    sessionAffinity: false,
    accounts: [
      {
        id: 'acc-1', name: 'team@google.com', provider: 'Antigravity', label: 'Team account',
        disabled: false, unavailable: false, resetAt: null,
        quotas: [{ label: '5-hour', remainingPercent: 72, resetAt: new Date(Date.now() + 3600_000).toISOString() }],
      },
      {
        id: 'acc-2', name: 'user@anthropic.com', provider: 'Claude', label: 'Claude Pro',
        disabled: false, unavailable: false, resetAt: null,
        quotas: [{ label: 'weekly', remainingPercent: 30, resetAt: null }],
      },
      {
        id: 'acc-3', name: 'user@openai.com', provider: 'Codex', label: 'Codex account',
        disabled: true, unavailable: false, resetAt: null,
        quotas: [],
      },
    ],
    models: [
      { id: 'claude-sonnet-4-20250514', ownedBy: 'anthropic' },
      { id: 'gemini-2.5-pro', ownedBy: 'google' },
    ],
    ...overrides,
  };
}

async function mockRouter(page: Page, snapshot: CliRouterSnapshot) {
  await page.route('**/api/cli-router', (route) => {
    if (route.request().method() === 'GET') {
      return route.fulfill({ json: snapshot });
    }
    return route.continue();
  });
}

// ---------------------------------------------------------------------------
// Connected state
// ---------------------------------------------------------------------------

test('renders connected panel with accounts, models, strategy, and provider tabs', async ({ mount, page }) => {
  const snapshot = makeSnapshot();
  await mockRouter(page, snapshot);

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('CLI Router')).toBeVisible();
  await expect(component.getByText('connected')).toBeVisible();

  // Accounts visible
  await expect(component.getByTestId('account-acc-1')).toBeVisible();
  await expect(component.getByTestId('account-acc-2')).toBeVisible();
  await expect(component.getByTestId('account-acc-3')).toBeVisible();

  await component.getByText('Router settings and models', { exact: true }).click();
  // Models list
  await expect(component.getByText('gah-router/claude-sonnet-4-20250514')).toBeVisible();
  await expect(component.getByText('gah-router/gemini-2.5-pro')).toBeVisible();

  // Strategy select
  const strategySelect = component.getByRole('combobox');
  await expect(strategySelect).toHaveValue('round-robin');

  // Provider filter tabs
  await expect(component.getByRole('button', { name: /All/ })).toBeVisible();
  await expect(component.getByRole('button', { name: /^Claude(?: \([0-9]+\))?$/ })).toBeVisible();
});

test('provider filter tabs filter the account list', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByTestId('account-acc-1')).toBeVisible();

  // Filter to Claude
  await component.getByRole('button', { name: /^Claude(?: \([0-9]+\))?$/ }).click();
  await expect(component.getByTestId('account-acc-1')).toHaveCount(0);
  await expect(component.getByTestId('account-acc-2')).toBeVisible();
  await expect(component.getByTestId('account-acc-3')).toHaveCount(0);

  // Filter back to All
  await component.getByRole('button', { name: /All/ }).click();
  await expect(component.getByTestId('account-acc-1')).toBeVisible();
  await expect(component.getByTestId('account-acc-3')).toBeVisible();
});

// ---------------------------------------------------------------------------
// Label privacy toggle
// ---------------------------------------------------------------------------

test('show labels checkbox reveals and hides account labels', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());

  const component = await mount(<CliRouterPanel />);
  // Email-bearing filenames stay hidden until the owner chooses to show them.
  await expect(component.getByText('team@google.com')).toHaveCount(0);
  await expect(component.getByText('Team account')).toHaveCount(0);

  // Click show labels
  await component.getByText('Show labels').click();
  await expect(component.getByText('Team account')).toBeVisible();
  // The original name appears as secondary text
  await expect(component.getByText('team@google.com')).toBeVisible();
});

// ---------------------------------------------------------------------------
// Account enable/disable
// ---------------------------------------------------------------------------

test('toggling account disabled state calls API and shows feedback', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  let statusCalls = 0;
  await page.route('**/api/cli-router/accounts/status', (route) => {
    statusCalls++;
    return route.fulfill({ json: makeSnapshot() });
  });

  const component = await mount(<CliRouterPanel />);
  const pausedAccount = component.getByTestId('account-acc-3');
  await expect(pausedAccount.getByText('Paused')).toBeVisible();

  await pausedAccount.getByRole('button', { name: /Enable/ }).click();
  await expect(pausedAccount.getByText('Enabled')).toBeVisible();
  expect(statusCalls).toBe(1);
});

test('account toggle failure shows error feedback', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  await page.route('**/api/cli-router/accounts/status', (route) => {
    return route.fulfill({ status: 500, json: { message: 'Internal error' } });
  });

  const component = await mount(<CliRouterPanel />);
  const account = component.getByTestId('account-acc-1');
  await account.getByRole('button', { name: /Pause/ }).click();
  await expect(account.getByRole('alert')).toContainText('Internal error');
});

// ---------------------------------------------------------------------------
// Account refresh
// ---------------------------------------------------------------------------

test('quota refresh button calls API and shows success', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  let refreshCalls = 0;
  await page.route('**/api/cli-router/accounts/refresh', (route) => {
    refreshCalls++;
    return route.fulfill({ json: makeSnapshot() });
  });

  const component = await mount(<CliRouterPanel />);
  const account = component.getByTestId('account-acc-1');
  await account.getByRole('button', { name: /Refresh quota/ }).click();
  await expect(account.getByText('Quota refreshed')).toBeVisible();
  expect(refreshCalls).toBe(1);
});

// ---------------------------------------------------------------------------
// Connection settings
// ---------------------------------------------------------------------------

test('connection form saves settings and clears password fields', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({ status: 'unconfigured', settings: { url: null, hasApiKey: false, hasManagementKey: false } }));
  let savedPayload: Record<string, unknown> | null = null;
  await page.route('**/api/cli-router/settings', (route) => {
    if (route.request().method() === 'PUT') {
      // The server's mutation guard rejects writes without an Idempotency-Key.
      if (!route.request().headers()['idempotency-key']) return route.fulfill({ status: 400, json: { message: 'idempotency_key_required' } });
      savedPayload = route.request().postDataJSON();
      return route.fulfill({ json: makeSnapshot() });
    }
    return route.continue();
  });

  const component = await mount(<CliRouterPanel />);
  // Unconfigured shows setup instructions
  await expect(component.getByText(/setup script/)).toBeVisible();

  // Fill connection form (expanded by default when unconfigured)
  await component.getByRole('textbox', { name: /Router URL/i }).fill('https://myrouter.example.com');
  await component.getByLabel(/API Key/i).fill('secret-api-key');
  await component.getByLabel(/Management Key/i).fill('secret-mgmt-key');

  await component.getByRole('button', { name: 'Save connection' }).click();
  await expect(component.getByText('Connection saved')).toBeVisible();

  // Password fields cleared after successful save
  await expect(component.getByLabel(/API Key/i)).toHaveValue('');
  await expect(component.getByLabel(/Management Key/i)).toHaveValue('');

  // Verify payload sent correctly (without printing actual secret values)
  expect(savedPayload).toHaveProperty('url', 'https://myrouter.example.com');
  expect(savedPayload).toHaveProperty('apiKey');
  expect(savedPayload).toHaveProperty('managementKey');
});

test('connection save failure shows error', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  await page.route('**/api/cli-router/settings', (route) => {
    return route.fulfill({ status: 400, json: { message: 'Invalid URL: must be HTTPS or loopback HTTP' } });
  });

  const component = await mount(<CliRouterPanel />);
  await component.getByText('Router settings and models', { exact: true }).click();
  await component.getByText('Connection settings').click();
  await component.getByRole('textbox', { name: /Router URL/i }).fill('http://external.example.com');
  await component.getByRole('button', { name: 'Save connection' }).click();
  await expect(component.getByRole('alert')).toContainText('Invalid URL');
});

// ---------------------------------------------------------------------------
// Strategy and session affinity
// ---------------------------------------------------------------------------

test('changing strategy calls routing API', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  let routingPayload: Record<string, unknown> | null = null;
  await page.route('**/api/cli-router/routing', (route) => {
    routingPayload = route.request().postDataJSON();
    return route.fulfill({ json: makeSnapshot({ strategy: 'fill-first' }) });
  });

  const component = await mount(<CliRouterPanel />);
  await component.getByText('Router settings and models', { exact: true }).click();
  await component.getByRole('combobox').selectOption('fill-first');
  await expect(component.getByText('Routing updated')).toBeVisible();
  expect(routingPayload).toEqual({ strategy: 'fill-first', sessionAffinity: false });
});

test('toggling session affinity calls routing API', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  let routingPayload: Record<string, unknown> | null = null;
  await page.route('**/api/cli-router/routing', (route) => {
    routingPayload = route.request().postDataJSON();
    return route.fulfill({ json: makeSnapshot({ sessionAffinity: true }) });
  });

  const component = await mount(<CliRouterPanel />);
  await component.getByText('Router settings and models', { exact: true }).click();
  await component.getByLabel('Session affinity').click();
  await expect(component.getByText('Routing updated')).toBeVisible();
  expect(routingPayload).toEqual({ strategy: 'round-robin', sessionAffinity: true });
});

test('routing write failure shows partial error', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  await page.route('**/api/cli-router/routing', (route) => {
    return route.fulfill({ status: 500, json: { message: 'Upstream timeout setting session-affinity' } });
  });

  const component = await mount(<CliRouterPanel />);
  await component.getByText('Router settings and models', { exact: true }).click();
  await component.getByRole('combobox').selectOption('weighted-round-robin');
  await expect(component.getByRole('alert')).toContainText('Upstream timeout');
});

// ---------------------------------------------------------------------------
// Unavailable / error states
// ---------------------------------------------------------------------------

test('unavailable status without an inventory shows the failure and no account rows', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({
    status: 'unavailable',
    accounts: [],
    models: [],
  }));

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('unavailable', { exact: true })).toBeVisible();
  await expect(component.getByRole('alert')).toContainText('Router unavailable');
  await expect(component.locator('[data-testid^="account-"]')).toHaveCount(0);
});

test('API fetch error shows error state with retry', async ({ mount, page }) => {
  await page.route('**/api/cli-router', (route) => {
    return route.fulfill({ status: 503, json: { message: 'Service Unavailable' } });
  });

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByRole('alert')).toContainText('Service Unavailable');
  await expect(component.getByRole('button', { name: 'Retry' })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Quota display
// ---------------------------------------------------------------------------

test('quota bars render real remaining percentages, not hardcoded values', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());

  const component = await mount(<CliRouterPanel />);
  const account = component.getByTestId('account-acc-1');
  await expect(account.getByText('72% remaining')).toBeVisible();
  const progress = account.locator('progress');
  await expect(progress).toHaveAttribute('value', '72');
  await expect(progress).toHaveAttribute('max', '100');
});

test('null quota shows Unknown, not 0%', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({
    accounts: [{
      id: 'acc-null', name: 'test@example.com', provider: 'Claude', label: '',
      disabled: false, unavailable: false, resetAt: null,
      quotas: [{ label: 'daily', remainingPercent: null, resetAt: null }],
    }],
  }));

  const component = await mount(<CliRouterPanel />);
  const account = component.getByTestId('account-acc-null');
  await expect(account.getByText('Unknown')).toBeVisible();
  await expect(account.locator('progress')).toHaveCount(0);
  await expect(component.getByText('Totals add reported account percentages', { exact: false })).toHaveCount(0);
  await expect(component.getByTestId('quota-summary-Claude').getByText('1 account', { exact: true })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Duplicate submit prevention
// ---------------------------------------------------------------------------

test('prevents duplicate form submissions', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({ status: 'unconfigured', settings: { url: null, hasApiKey: false, hasManagementKey: false } }));
  let saveCount = 0;
  await page.route('**/api/cli-router/settings', async (route) => {
    saveCount++;
    await new Promise((r) => setTimeout(r, 500));
    return route.fulfill({ json: makeSnapshot() });
  });

  const component = await mount(<CliRouterPanel />);
  await component.getByRole('textbox', { name: /Router URL/i }).fill('https://router.example.com');
  await component.getByLabel(/API Key/i).fill('key');
  await component.getByLabel(/Management Key/i).fill('mgmt');

  const saveButton = component.locator('button[type="submit"]');
  await saveButton.click();
  // Button should show saving state
  await expect(saveButton).toBeDisabled();
  await expect(saveButton).toHaveText('Saving…');
  // Second click should be prevented (force:true to bypass Playwright's disabled-check)
  await saveButton.click({ force: true });

  await expect(component.getByText('Connection saved')).toBeVisible();
  expect(saveCount).toBe(1);
});

// ---------------------------------------------------------------------------
// Mobile responsiveness
// ---------------------------------------------------------------------------

test('panel is usable at mobile viewport', async ({ mount, page }, testInfo) => {
  await mockRouter(page, makeSnapshot());
  await page.setViewportSize({ width: 390, height: 844 });

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('CLI Router')).toBeVisible();
  await expect(component.getByTestId('account-acc-1')).toBeVisible();

  // No horizontal overflow
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

  await page.screenshot({ path: testInfo.outputPath('cli-router-mobile.png'), fullPage: true });
});

// ---------------------------------------------------------------------------
// Model ID copy format
// ---------------------------------------------------------------------------

test('model IDs use gah-router/<id> format', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());

  const component = await mount(<CliRouterPanel />);
  await component.getByText('Router settings and models', { exact: true }).click();
  await expect(component.getByText('gah-router/claude-sonnet-4-20250514')).toBeVisible();
  await expect(component.getByText('gah-router/gemini-2.5-pro')).toBeVisible();
});

// ---------------------------------------------------------------------------
// Visible when quota unavailable
// ---------------------------------------------------------------------------

test('panel visible even when wrapped in error context', async ({ mount, page }) => {
  // Simulate: quota API fails, but router still works
  await page.route('**/api/quota*', (route) => route.fulfill({ status: 500, json: { message: 'Quota service down' } }));
  await mockRouter(page, makeSnapshot());

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('CLI Router')).toBeVisible();
  await expect(component.getByText('connected')).toBeVisible();
});

// ---------------------------------------------------------------------------
// Account with quotaError
// ---------------------------------------------------------------------------

test('account quota error is displayed', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({
    accounts: [{
      id: 'acc-err', name: 'user@example.com', provider: 'Codex', label: '',
      disabled: false, unavailable: false, resetAt: null,
      quotas: [],
      quotaError: 'Unsupported provider for quota check',
    }],
  }));

  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('Unsupported provider for quota check')).toBeVisible();
});

test('partial router outages preserve supplied account inventory and its unknown quotas', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot({ status: 'unavailable' }));
  const component = await mount(<CliRouterPanel />);
  await expect(component.getByText('unavailable', { exact: true })).toBeVisible();
  await expect(component.getByTestId('account-acc-1')).toBeVisible();
  await expect(component.getByTestId('account-acc-3')).toBeVisible();
  await expect(component.getByTestId('account-acc-3').getByText(/Quota not checked/)).toBeVisible();
});

test('connection loss retains known accounts until a successful empty inventory confirms removal', async ({ mount, page }) => {
  let snapshot = makeSnapshot();
  await page.route('**/api/cli-router', route => route.fulfill({ json: snapshot }));
  const component = await mount(<CliRouterPanel />);
  await expect(component.getByTestId('account-acc-1')).toBeVisible();
  snapshot = makeSnapshot({ status: 'unavailable', accounts: [] });
  await component.getByRole('button', { name: 'Refresh router', exact: true }).click();
  await expect(component.getByRole('alert')).toContainText('Router unavailable');
  await expect(component.getByTestId('account-acc-1')).toBeVisible();
  snapshot = makeSnapshot({ accounts: [] });
  await component.getByRole('button', { name: 'Refresh router', exact: true }).click();
  await expect(component.getByText('No router accounts configured.')).toBeVisible();
  await expect(component.getByTestId('account-acc-1')).toHaveCount(0);
});

test('account refresh keeps newly read quotas when the models or routing read fails', async ({ mount, page }) => {
  await mockRouter(page, makeSnapshot());
  const updated = makeSnapshot({ status: 'unavailable' });
  updated.accounts[0].quotas[0].remainingPercent = 42;
  await page.route('**/api/cli-router/accounts/refresh', route => route.fulfill({ json: updated }));
  const component = await mount(<CliRouterPanel />);
  const account = component.getByTestId('account-acc-1');
  await expect(account.getByText('72% remaining')).toBeVisible();
  await account.getByRole('button', { name: /Refresh quota/ }).click();
  await expect(account.getByText('42% remaining')).toBeVisible();
  await expect(component.getByRole('alert')).toContainText('Router unavailable');
});


test('provider summaries keep windows separate, omit unknown denominators, and include new providers', async ({ mount, page }) => {
  const accounts: CliRouterSnapshot['accounts'] = [
    { id: 'c1', name: 'Claude one', provider: 'Claude', label: '', disabled: false, unavailable: false, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: 30, resetAt: null }, { label: '5-hour', remainingPercent: 80, resetAt: null }] },
    { id: 'c2', name: 'Claude two', provider: 'Claude', label: '', disabled: true, unavailable: false, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: 60, resetAt: null }] },
    { id: 'c3', name: 'Claude three', provider: 'Claude', label: '', disabled: false, unavailable: true, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: null, resetAt: null }] },
    { id: 'x1', name: 'xAI one', provider: 'xAI', label: '', disabled: false, unavailable: false, resetAt: null, quotas: [] },
  ];
  await mockRouter(page, makeSnapshot({ accounts }));
  const component = await mount(<CliRouterPanel />);
  const summary = component.getByTestId('quota-summary-Claude');
  await expect(summary.getByText('90%', { exact: true })).toBeVisible();
  await expect(summary.getByText('of 200%', { exact: true })).toBeVisible();
  await expect(summary.getByText('2/3 accounts reported', { exact: true })).toBeVisible();
  await expect(summary.getByText('80% of 100% · 1/3 reported', { exact: true })).toBeVisible();
  await component.getByRole('button', { name: 'xAI (1)', exact: true }).click();
  await expect(component.getByTestId('account-x1')).toBeVisible();
  await expect(component.getByTestId('account-c1')).toHaveCount(0);
  await expect(component.getByTestId('quota-summary-xAI').getByText('Quota not checked')).toBeVisible();
  await expect(component.getByTestId('quota-summary-xAI').getByRole('progressbar')).toHaveCount(0);
});


test('summary identities stay masked and the next reset excludes elapsed timestamps', async ({ mount, page }) => {
  const snapshot = makeSnapshot({ accounts: [
    { id: 'private-owner@example.com', name: 'private-owner@example.com', provider: 'Claude', label: 'Owner label', disabled: false, unavailable: false, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: 30, resetAt: new Date(Date.now() - 3600_000).toISOString() }] },
    { id: 'private-second@example.com', name: 'private-second@example.com', provider: 'Claude', label: 'Second label', disabled: false, unavailable: false, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: 60, resetAt: new Date(Date.now() + 3600_000).toISOString() }, { label: 'daily', remainingPercent: null, resetAt: null }] },
    { id: 'private-unknown@example.com', name: 'private-unknown@example.com', provider: 'Claude', label: '', disabled: false, unavailable: false, resetAt: null, quotas: [{ label: 'weekly', remainingPercent: null, resetAt: null }] },
  ] });
  await mockRouter(page, snapshot);
  const component = await mount(<CliRouterPanel />);
  const summary = component.getByTestId('quota-summary-Claude');
  await expect(summary.getByText(/Next reset in (59m|1h 0m)/)).toBeVisible();
  const identities = await summary.locator('[aria-label], [title]').evaluateAll(elements => elements.map(element => `${element.getAttribute('aria-label') ?? ''} ${element.getAttribute('title') ?? ''}`).join(' '));
  expect(identities).not.toContain('private-owner@example.com');
  expect(identities).not.toContain('private-second@example.com');
  expect(identities).not.toContain('private-unknown@example.com');
  await expect(summary.getByRole('progressbar', { name: 'Claude · private-, weekly: 30% remaining', exact: true })).toBeVisible();
  await component.getByText('Show labels').click();
  await expect(summary.getByRole('progressbar', { name: 'Owner label, weekly: 30% remaining', exact: true })).toBeVisible();
});
