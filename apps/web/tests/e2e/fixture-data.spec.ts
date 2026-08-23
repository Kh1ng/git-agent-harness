import { expect, test, type Page } from '@playwright/test';

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

async function navigateTo(page: Page, label: string) {
  await page.getByRole('button', { name: label, exact: true }).click();
}

test('Overview renders fixture profile + status data from the hermetic server', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('Profile: Fixture', { exact: false }).first()).toBeVisible();
  // fixture status.json carries 42 total ledger entries.
  await expect(page.getByText('42', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Loop running', { exact: true })).toBeVisible();
});

test('Quota page renders the fixture quota snapshot observations', async ({ page }) => {
  await page.goto('/');
  await navigateTo(page, 'Quota');
  // responses/quota.json carries codex/claude candidate cards with usage.
  await expect(page.getByText('codex', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('claude', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('Windows', { exact: true })).toBeVisible();
  await expect(page.getByText('weekly', { exact: false })).toBeVisible();
  await expect(page.getByText(/66% remaining/)).toBeVisible();
});

test('Telemetry page renders a backend row from the fixture report', async ({ page }) => {
  await page.goto('/');
  await navigateTo(page, 'Telemetry');
  // responses/report.json carries codex + claude comparison rows.
  await expect(page.getByText('codex', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('claude', { exact: false }).first()).toBeVisible();
});

test('Settings profile section lists the fixture profile', async ({ page }) => {
  await page.goto('/');
  await navigateTo(page, 'Settings');
  // responses/profile-list.json contains the synthetic 'fixture' profile,
  // rendered as "Fixture (fixture)" in the profile selector.
  const profileSelect = page
    .locator('section')
    .filter({ hasText: 'Which configured GAH repo' })
    .getByRole('combobox');
  await expect(profileSelect).toContainText('Fixture');
});
