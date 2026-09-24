import { expect, test } from '@playwright/test';

const usageRollup = {
  profile: 'fixture',
  since: Date.parse('2026-09-16T00:00:00Z'),
  generated_at: Date.parse('2026-09-23T00:00:00Z'),
  rows: [{
    backend: 'claude',
    model: 'claude-opus-4-1',
    day: '2026-09-23',
    turns: 3,
    input_tokens: 4_000_000,
    output_tokens: 2_160_000,
    total_tokens: 6_160_000,
    estimated_cost_usd: null
  }],
  unattributed_turns: 1,
  usage_unavailable: [{
    session_id: 'codex-test-chat',
    backend: 'codex',
    model: 'gpt-5.3',
    day: '2026-09-23'
  }],
  tickets: []
};

test('empty dispatch telemetry keeps manager-chat usage and unavailable-turn identity visible', async ({ page }) => {
  await page.route('**/api/usage/rollup**', route => route.fulfill({ json: usageRollup }));
  await page.route('**/api/report?**', route => route.fulfill({ json: { comparisons: [] } }));
  await page.route('**/api/report/series?**', route => route.fulfill({ json: { series: [], bucket: 'daily' } }));

  await page.goto('/');
  await page.getByRole('button', { name: 'Telemetry', exact: true }).click();

  const managerUsage = page.locator('section').filter({ hasText: 'Manager chat usage' });
  await expect(managerUsage.getByText('claude', { exact: true })).toBeVisible();
  await expect(managerUsage.getByText('claude-opus-4-1 · 6.2M · 3 turns', { exact: true })).toBeVisible();
  await expect(managerUsage.getByRole('heading', { name: 'Tokens by backend and model' })).toBeVisible();
  await expect(managerUsage.getByRole('progressbar', { name: 'claude claude-opus-4-1: 6.2M tokens across 3 turns' })).toBeVisible();
  await expect(managerUsage.getByRole('heading', { name: 'Usage unavailable (1)' })).toBeVisible();
  await expect(managerUsage.getByText('codex-test-chat', { exact: true })).toBeVisible();
  await expect(managerUsage.getByText('codex', { exact: true })).toBeVisible();
  await expect(managerUsage.getByText('gpt-5.3', { exact: true })).toBeVisible();

  await expect(page.getByText('No dispatch-ledger trend', { exact: true })).toBeVisible();
  await expect(page.getByText('No dispatch-ledger data', { exact: true })).toBeVisible();

  await page.reload();
  await page.getByRole('button', { name: 'Telemetry', exact: true }).click();
  await expect(page.getByText('claude-opus-4-1 · 6.2M · 3 turns', { exact: true })).toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(managerUsage.getByText('codex-test-chat', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
