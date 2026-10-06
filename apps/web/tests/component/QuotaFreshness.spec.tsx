import { readFileSync } from 'node:fs';
import type { QuotaSnapshot, AccountUsageObservation } from '@git-agent-harness/contracts';
import { MockStoreProvider } from '../../src/test-utils/MockStoreProvider.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';
import { test, expect } from "@playwright/experimental-ct-react";
import { QuotaPage, QuotaFreshnessPanel } from "../../src/pages/QuotaPage.js";
import React from "react";

test.beforeEach(async ({ page }) => {
  await page.route('**/api/registry/quota**', route => route.fulfill({ json: { profile: 'test-profile', since: '7d', nodes: [] } }));
});

test('named API connections remain visible when their provider has no usage reading', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  quota.candidates = [];
  quota.quota_checks = [
    { backend: 'opencode', provider: 'google', backend_instance: 'credential:google-personal', credential_id: 'google-personal', status: 'no_data', checked_at: new Date().toISOString() },
    { backend: 'opencode', provider: 'google', backend_instance: 'credential:google-work', credential_id: 'google-work', status: 'no_data', checked_at: new Date().toISOString() },
  ];
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  await expect(component.getByTestId('quota-candidate-opencode-0').getByText(/credential:google-personal/)).toBeVisible();
  await expect(component.getByTestId('quota-candidate-opencode-1').getByText(/credential:google-work/)).toBeVisible();
  await expect(component.getByText('This connection has no provider usage reading. Its allowance is unknown.')).toHaveCount(2);
  await expect(component.getByRole('progressbar')).toHaveCount(0);
});

test('two credentials for the same verified account count one allowance', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  quota.candidates = [];
  const checked = new Date().toISOString();
  const account: AccountUsageObservation = { account_id: 'same-organization', workspace_id: null, period_start: '2026-10-01T00:00:00Z', period_end: checked, currency: 'USD', models: [] };
  quota.quota_checks = ['personal', 'work'].map(id => ({
    backend: 'mistral-dashboard', provider: 'mistral', backend_instance: `credential:${id}`, credential_id: id,
    quota_pool: 'mistral-dashboard:verified-same-account', status: 'data', checked_at: checked,
    quota_observations: [{ backend: 'mistral-dashboard', credential_id: id, quota_window: 'monthly', observed_at: checked, quota_remaining_percent: 25, account_usage: account }]
  }));
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  const summary = component.getByLabel('Configured provider summaries');
  await expect(summary.getByText('1 account observation · 1 with quota percentages', { exact: true })).toBeVisible();
  await expect(component.getByTestId('quota-candidate-mistral-dashboard-0')).toBeVisible();
  await expect(component.getByTestId('quota-candidate-mistral-dashboard-1')).toBeVisible();
});

test("renders recent no-data checks separately from stale quota data and exposes failures", async ({
  mount,
}) => {
  const recent = new Date().toISOString();
  const stale = new Date(Date.now() - 3 * 24 * 60 * 60 * 1000).toISOString();
  const component = await mount(
    <QuotaFreshnessPanel
      generatedAt={recent}
      freshness={{ quota_checked_at: recent, quota_observed_at: stale }}
      quotaChecks={[
        { backend: "codex", checked_at: recent, status: "no_data" },
        {
          backend: "vibe",
          checked_at: stale,
          status: "failed",
          error: "Mistral Admin API unavailable",
        },
      ]}
    />,
  );

  await expect(
    component.getByText("Account quota check", { exact: true }),
  ).toBeVisible();
  await expect(
    component.getByText("Quota data", { exact: true }),
  ).toBeVisible();
  await expect(component.getByText("No quota data recorded")).toHaveClass(
    /badge-unknown/,
  );
  await expect(component.getByText("Check failed")).toBeVisible();
  await expect(
    component.getByText("Mistral Admin API unavailable"),
  ).toBeVisible();
  await expect(
    component.getByTestId("quota-check-codex").getByText("Stale"),
  ).toHaveCount(0);
  await expect(
    component.getByTestId("quota-check-vibe").getByText("Stale"),
  ).toBeVisible();
});


test('OpenCode candidates show their provider and instance instead of a shared runner balance', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  const template = quota.candidates[0];
  quota.candidates = [
    { ...template, backend: 'opencode', provider: 'nous', backend_instance: 'nous-api', model: 'nous-portal/openai/gpt-5.6-luna', quota_pool: 'nous-api' },
    { ...template, backend: 'opencode', provider: 'mistral', backend_instance: 'mistral-api', model: 'mistral/devstral', quota_pool: 'mistral-api' },
    { ...template, backend: 'opencode', backend_instance: 'cli-router', model: 'gah-router/gemini-3.1-pro-low', quota_pool: 'cli-router' }
  ];
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  await expect(component.getByText('Nous / nous-api / nous-portal/openai/gpt-5.6-luna', { exact: true })).toBeVisible();
  await expect(component.getByText('Mistral / mistral-api / mistral/devstral', { exact: true })).toBeVisible();
  await expect(component.getByText('CLI subscription router / cli-router / gah-router/gemini-3.1-pro-low', { exact: true })).toBeVisible();
});

test('account instances remain visible without quota readings and AGY accounts share a provider', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  const template = { ...quota.candidates[0], observed_at: null, eligible_now: true, quota_observations: [] };
  quota.candidates = [
    { ...template, backend: 'agy', provider: 'antigravity', backend_instance: 'agy:external', model: null },
    { ...template, backend: 'agy-second', provider: 'antigravity', backend_instance: 'agy-second', model: null },
    { ...template, backend: 'vibe', provider: 'mistral', backend_instance: 'vibe-monthly', model: null },
    { ...template, backend: 'vibe', provider: 'mistral', backend_instance: 'vibe-second', model: null }
  ];
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  const filters = component.getByRole('group', { name: 'Filter candidates by provider' });
  await expect(filters.getByRole('button', { name: 'Antigravity (2)', exact: true })).toBeVisible();
  await expect(filters.getByRole('button', { name: 'agy-second (1)', exact: true })).toHaveCount(0);
  for (const instance of ['agy:external', 'agy-second', 'vibe-monthly', 'vibe-second']) {
    await expect(component.getByText(new RegExp(instance)).first()).toBeVisible();
  }
  await expect(component.getByText('Quota unknown', { exact: true })).toHaveCount(4);
  await expect(component.getByText('Availability unverified', { exact: true })).toHaveCount(4);
  await expect(component.getByText('Eligible', { exact: true })).toHaveCount(0);
  await expect(component.getByRole('progressbar')).toHaveCount(0);
  await filters.getByRole('button', { name: 'Mistral (2)', exact: true }).click();
  await expect(component.getByText(/vibe-second/).first()).toBeVisible();
  await expect(component.getByTestId('quota-candidate-agy-0')).toHaveCount(0);
});

test('billing provider failures are visible without opening diagnostics and unknown OpenCode providers are not invented', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  quota.candidates = [
    { ...quota.candidates[0], backend: 'opencode', provider: 'nous', backend_instance: 'nous-api', model: 'nous-portal/openai/gpt-5.6-luna', quota_observations: [] },
    { ...quota.candidates[0], backend: 'opencode', provider: null, backend_instance: 'unidentified', model: null, quota_observations: [] }
  ];
  quota.quota_checks = [{ backend: 'opencode', provider: 'nous', backend_instance: 'nous-api', checked_at: new Date().toISOString(), status: 'failed', error: 'Nous usage is unavailable with this inference API key' }];
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  await expect(component.getByRole('alert')).toContainText('Nous / nous-api · Quota check failed: Nous usage is unavailable with this inference API key');
  const filters = component.getByRole('group', { name: 'Filter candidates by provider' });
  await expect(filters.getByRole('button', { name: 'Unknown provider (1)', exact: true })).toBeVisible();
  await expect(filters.getByRole('button', { name: 'opencode (1)', exact: true })).toHaveCount(0);
  await expect(component.getByText(/OpenCode runner/)).toHaveCount(2);
  await expect(component.getByRole('progressbar')).toHaveCount(0);
});

test('worker allowances keep node identity, independent instances and central usage separate', async ({ mount, page }) => {
  const central = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  central.usage.total_tokens = 1200;
  const template = { ...central.candidates[0], observed_at: null, quota_observations: [] };
  central.candidates = [{ ...template, backend: 'claude', provider: 'anthropic', backend_instance: 'claude', model: null }];
  const worker: QuotaSnapshot = {
    ...central,
    candidates: [
      { ...template, backend: 'claude', provider: 'anthropic', backend_instance: 'claude', model: null, quota_observations: [
        { backend: 'claude', quota_window: '5-hour', quota_remaining_percent: 100, observed_at: new Date().toISOString(), usage_source: 'claude_oauth_usage' },
        { backend: 'claude', quota_window: 'weekly', quota_remaining_percent: 22, observed_at: new Date().toISOString(), usage_source: 'claude_oauth_usage' }
      ] },
      { ...template, backend: 'vibe', provider: 'mistral', backend_instance: 'vibe-second', model: null }
    ]
  };
  await page.route('**/api/registry/quota**', route => route.fulfill({ json: { profile: 'test-profile', since: '7d', nodes: [
    { nodeId: 'mac-worker', displayName: 'Mac worker', state: 'available', quota: worker },
    { nodeId: 'offline-worker', displayName: 'Offline worker', state: 'unavailable', quota: null, error: 'Worker quota check failed.' }
  ] } }));
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={central}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  const mac = component.getByTestId('node-quota-mac-worker');
  await expect(mac.getByText('Mac worker', { exact: true })).toBeVisible();
  await expect(mac.getByText(/Node mac-worker/)).toBeVisible();
  await expect(mac.getByRole('progressbar', { name: 'weekly: 78% used, 22% remaining' })).toBeVisible();
  await expect(mac.getByRole('progressbar', { name: '5-hour: 0% used, 100% remaining' })).toBeVisible();
  await expect(mac.getByText(/source: claude_oauth_usage/)).toHaveCount(2);
  await expect(mac.getByText(/vibe-second/).first()).toBeVisible();
  await expect(mac.getByText('Quota unknown', { exact: true })).toHaveCount(1);
  await expect(component.getByText('Quota unknown', { exact: true })).toHaveCount(2);
  await expect(component.getByRole('progressbar')).toHaveCount(2);
  await expect(component.getByTestId('node-quota-offline-worker').getByText('Worker quota check failed.')).toBeVisible();
  await component.getByText('Usage and data freshness', { exact: true }).click();
  await expect(component.locator('.stat-tile').filter({ hasText: 'Usage (7d)' }).getByText('1.2k', { exact: true })).toBeVisible();
});

test('observation-only Nous accounts show balances without becoming generic OpenCode routing capacity', async ({ mount, page }) => {
  const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
  quota.candidates = [{ ...quota.candidates[0], backend: 'opencode', provider: null, backend_instance: 'opencode', model: null, quota_observations: [] }];
  quota.quota_checks = [{ backend: 'opencode', provider: 'nous', backend_instance: 'opencode:nous-portal-api', quota_pool: 'nous-portal-api', checked_at: new Date().toISOString(), status: 'data', quota_observations: [{ backend: 'opencode', quota_window: 'subscription-monthly', quota_remaining_percent: 0, observed_at: new Date().toISOString(), usage_source: 'nous_portal_account' }] }];
  await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
  const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  const nous = component.getByTestId('quota-candidate-opencode-0').filter({ hasText: 'Observed account' });
  await expect(nous.getByRole('progressbar', { name: 'subscription-monthly: 100% used, 0% remaining' })).toBeVisible();
  await expect(nous.getByText('Availability unverified', { exact: true })).toBeVisible();
  await expect(nous.getByText('Eligible', { exact: true })).toHaveCount(0);
  await expect(nous.getByText('Usage details', { exact: true })).toHaveCount(0);
  await expect(component.getByTestId('quota-candidate-opencode-0').filter({ hasNotText: 'Observed account' }).getByRole('progressbar')).toHaveCount(0);

  const failedQuota: QuotaSnapshot = {
    ...quota,
    quota_checks: [
      { ...quota.quota_checks[0], status: 'failed', quota_observations: [], error: 'Nous account check failed' },
      { backend: 'vibe', provider: 'mistral', backend_instance: 'vibe-second', quota_pool: 'vibe-second-monthly', checked_at: new Date().toISOString(), status: 'failed', error: 'Vibe account check failed' },
      { backend: 'claude', provider: 'anthropic', backend_instance: 'claude-unverified', checked_at: new Date().toISOString(), status: 'no_data' }
    ]
  };
  await component.update(<MockStoreProvider statusData={null} quotaData={failedQuota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
  await expect(nous.getByText('Quota unknown', { exact: true })).toBeVisible();
  await expect(nous.getByText('Availability unverified', { exact: true })).toBeVisible();
  await expect(nous.getByText('Usage details', { exact: true })).toHaveCount(0);
  const vibe = component.getByTestId('quota-candidate-vibe-0');
  await expect(vibe.getByText(/vibe-second/).first()).toBeVisible();
  await expect(vibe.getByText('Quota unknown', { exact: true })).toBeVisible();
  await expect(vibe.getByText('Availability unverified', { exact: true })).toBeVisible();
  await expect(vibe.getByText('Usage details', { exact: true })).toHaveCount(0);
  await expect(component.getByRole('progressbar')).toHaveCount(0);
  const filters = component.getByRole('group', { name: 'Filter candidates by provider' });
  await expect(filters.getByRole('button', { name: 'Nous (1)', exact: true })).toBeVisible();
  await expect(filters.getByRole('button', { name: 'Mistral (1)', exact: true })).toBeVisible();
  await expect(filters.getByRole('button', { name: 'Anthropic (1)', exact: true })).toHaveCount(0);
  await expect(component.getByTestId('quota-candidate-claude-0')).toHaveCount(0);
});

for (const width of [390, 1440]) {
  test(`dashboard consumption retains organization scope without assigning Vibe instances a quota at ${width}px`, async ({ mount, page }) => {
    await page.setViewportSize({ width, height: 950 });
    const quota = JSON.parse(readFileSync(new URL('../../../server/tests/fixtures/gah/responses/quota.json', import.meta.url), 'utf8')) as QuotaSnapshot;
    quota.usage.total_tokens = 1200;
    const template = { ...quota.candidates[0], backend: 'vibe', provider: 'mistral', model: null, observed_at: null, quota_observations: [] };
    quota.candidates = [{ ...template, backend_instance: 'vibe-first' }, { ...template, backend_instance: 'vibe-second' }];
    const accountUsage: AccountUsageObservation = {
      account_id: 'customer-test', workspace_id: null, period_start: '2026-10-01T00:00:00Z', period_end: '2026-10-03T00:00:00Z', currency: 'USD',
      requests: 42, input_tokens: 300, cached_input_tokens: 1000, output_tokens: 0, cost: 12.34, cost_source: 'dashboard_prices',
      models: [
        { model: 'Vibe display alias', usage_type: 'vibe', requests: 40, input_tokens: 300, cached_input_tokens: 1000, output_tokens: 0, cost: 12.32 },
        { model: 'web_search', usage_type: 'vibe_connectors', requests: 2 }
      ]
    };
    quota.quota_checks = [{ backend: 'mistral-dashboard', backend_instance: 'mistral-dashboard:organization-test', quota_pool: 'mistral-dashboard:organization-test', status: 'data', checked_at: new Date().toISOString(), quota_observations: [{ backend: 'mistral-dashboard', observed_at: new Date().toISOString(), usage_source: 'mistral_dashboard_session', account_usage: accountUsage }] }];
    await page.route('**/api/cli-router', route => route.fulfill({ json: { settings: { url: null, hasApiKey: false, hasManagementKey: false }, status: 'unconfigured', strategy: 'round-robin', sessionAffinity: false, accounts: [], models: [] } }));
    const component = await mount(<MockStoreProvider statusData={null} quotaData={quota}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
    const dashboard = component.getByTestId('quota-candidate-mistral-dashboard-2');
    await expect(component.getByRole('button', { name: 'Mistral (3)', exact: true })).toBeVisible();
    const usage = dashboard.getByTestId('provider-account-usage');
    await expect(usage.getByText('Organization scope · Account customer-test', { exact: true })).toBeVisible();
    await expect(usage.getByText(/Period .*2026.*2026.*\(UTC\)/)).toBeVisible();
    await expect(usage.getByText('42', { exact: true })).toBeVisible();
    await expect(usage.getByText('$12.34', { exact: true })).toBeVisible();
    await expect(usage.getByText('1,000', { exact: true }).first()).toBeVisible();
    await expect(usage.getByText('0', { exact: true }).first()).toBeVisible();
    await expect(usage.getByText(/Consumption priced at dashboard rates/)).toBeVisible();
    await usage.getByText('Usage by model (2)', { exact: true }).click();
    await expect(usage.getByText('Vibe display alias · Vibe', { exact: true })).toBeVisible();
    await expect(usage.getByText('web_search · Vibe connector', { exact: true })).toBeVisible();
    await expect(usage.getByText('Unknown', { exact: true })).toBeVisible();
    await expect(dashboard.getByText('Availability unverified', { exact: true })).toBeVisible();
    await expect(dashboard.getByText('Quota unknown', { exact: true })).toBeVisible();
    await expect(dashboard.getByText('Usage details', { exact: true })).toHaveCount(0);
    for (const index of [0, 1]) {
      const candidate = component.getByTestId(`quota-candidate-vibe-${index}`);
      await expect(candidate.getByText('Quota unknown', { exact: true })).toBeVisible();
      await expect(candidate.getByTestId('provider-account-usage')).toHaveCount(0);
    }
    await expect(component.getByRole('progressbar')).toHaveCount(0);
    await component.getByText('Usage and data freshness', { exact: true }).click();
    await expect(component.locator('.stat-tile').filter({ hasText: 'Usage (7d)' }).getByText('1.2k', { exact: true })).toBeVisible();
    const withAllowance: QuotaSnapshot = { ...quota, quota_checks: [{ ...quota.quota_checks[0], quota_observations: [{ ...quota.quota_checks[0].quota_observations![0], quota_window: 'vibe-code-included-monthly', quota_remaining_percent: 40, quota_reset_at: '2026-11-01T00:00:00Z' }] }] };
    await component.update(<MockStoreProvider statusData={null} quotaData={withAllowance}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
    await expect(dashboard.getByRole('progressbar', { name: 'Vibe Code included monthly allowance: 60% used, 40% remaining' })).toBeVisible();
    await expect(usage.getByText('$12.34', { exact: true })).toBeVisible();
    await expect(dashboard.getByText(/Resets/)).toBeVisible();
    await expect(dashboard.getByText('Quota unknown', { exact: true })).toHaveCount(0);
    await expect(component.getByRole('progressbar')).toHaveCount(1);
    await expect(component.getByTestId('quota-candidate-vibe-0').getByRole('progressbar')).toHaveCount(0);
    await expect(component.getByTestId('quota-candidate-vibe-1').getByRole('progressbar')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: `test-results/component/mistral-dashboard-${width}.png`, fullPage: true });
    const allowanceUnavailable: QuotaSnapshot = { ...withAllowance, quota_checks: [{ ...withAllowance.quota_checks[0], quota_observations: [{ ...withAllowance.quota_checks[0].quota_observations![0], quota_remaining_percent: undefined, quota_reset_at: undefined }] }] };
    await component.update(<MockStoreProvider statusData={null} quotaData={allowanceUnavailable}><WebSocketProvider><QuotaPage /></WebSocketProvider></MockStoreProvider>);
    await expect(usage.getByText('$12.34', { exact: true })).toBeVisible();
    await expect(dashboard.getByText('Monthly allowance reading unavailable', { exact: true })).toBeVisible();
    await expect(dashboard.getByText('Quota unknown', { exact: true })).toBeVisible();
    await expect(component.getByRole('progressbar')).toHaveCount(0);
    await expect(dashboard.getByText(/Resets/)).toHaveCount(0);
  });
}
