import { expect, test, type Page } from '@playwright/test';
import { openUsage } from './helpers/navigation.js';

/**
 * Issue #636 AC3: with the fixture-backed server running (see
 * playwright.config.ts webServer), fixture data actually renders -- the e2e
 * is hermetic and deterministic, not just structural. The fixture's
 * responses/*.json are recorded from the real Rust gah binary, so these
 * assertions prove the pipeline fixture -> gahCli -> REST -> React actually
 * renders, not merely that the page doesn't crash.
 *
 * The web app is state-routed (App.tsx currentPage), not URL-routed, so each
 * test clicks the nav button like the smoke spec does rather than page.goto.
 */

async function navigateTo(page: Page, label: string, tab?: string) {
  await page.getByRole('button', { name: label, exact: true }).click();
  if (tab) await page.getByRole('navigation', { name: 'Page tabs' }).getByRole('button', { name: tab, exact: true }).click();
}

test('Overview renders fixture profile + status data from the hermetic server', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('Profile: Fixture', { exact: false }).first()).toBeVisible();
  // fixture status.json carries 42 total ledger entries.
  await expect(page.getByText('42', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Factory on', { exact: true })).toBeVisible();
});

test('Quota page renders the fixture quota snapshot observations', async ({ page }) => {
  await page.goto('/');
  await openUsage(page, 'Quota');
  // responses/quota.json carries codex/claude candidate ledger rows with quota windows.
  await expect(page.getByText('codex', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('claude', { exact: false }).first()).toBeVisible();
  await expect(page.getByTestId('quota-candidate-codex-0').getByRole('progressbar', { name: 'weekly · codex-mini: 34.2% used, 65.8% remaining', exact: true })).toBeVisible();
  await expect(page.getByText('weekly', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('65.8% remaining', { exact: true })).toBeVisible();
  await page.getByText('Usage and data freshness', { exact: true }).click();
  await expect(page.getByText('Account quota check', { exact: true })).toBeVisible();
  await expect(page.getByText('Quota data', { exact: true })).toBeVisible();
  await expect(page.getByTestId('quota-check-codex').getByText('No quota data recorded')).toBeVisible();
  await expect(page.getByTestId('quota-check-codex').getByText('Stale')).toHaveCount(0);
  await expect(page.getByTestId('quota-check-vibe').getByText('Check failed')).toBeVisible();
  await expect(page.getByTestId('quota-check-vibe').getByText('Stale')).toBeVisible();
});

test('Telemetry page renders a backend row from the fixture report', async ({ page }) => {
  await page.goto('/');
  await openUsage(page, 'Telemetry');
  // responses/report.json carries codex + claude comparison rows.
  await expect(page.getByText('codex', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('claude', { exact: false }).first()).toBeVisible();

  const chatUsage = page.locator('section').filter({ hasText: 'Manager chat usage' });
  await expect(chatUsage.getByText('gpt-5.3-codex · 3.0k · 4 turns', { exact: true })).toBeVisible();
  await expect(chatUsage.getByRole('columnheader', { name: 'Cache / other' })).toBeVisible();
  await expect(chatUsage.getByText(/\$0\.0500 API equivalent/)).toBeVisible();
});

test('the project switcher lists the fixture profile', async ({ page }) => {
  await page.goto('/');
  // responses/profile-list.json contains the synthetic 'fixture' profile.
  await expect(page.getByRole('button', { name: /^Project: Fixture/ })).toBeVisible();
  await page.getByRole('button', { name: /^Project:/ }).click();
  await expect(page.getByRole('menu', { name: 'Projects' }).getByRole('menuitemradio', { name: /^Fixture/ })).toHaveAttribute('aria-checked', 'true');
});
