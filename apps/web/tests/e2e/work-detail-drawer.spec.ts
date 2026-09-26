import { expect, test } from '@playwright/test';

test('opens work details and acts without leaving Factory', async ({ page }) => {
  await page.route('**/api/work/**', (route) => route.fulfill({ json: [] }));
  await page.route('**/api/hold/set', (route) => route.fulfill({ json: { success: true } }));
  await page.goto('/');
  await page.getByRole('button', { name: 'Factory', exact: true }).click();
  const before = page.url();
  await page.getByRole('button', { name: 'View details', exact: true }).first().click();
  const drawer = page.getByRole('dialog');
  await expect(drawer).toBeVisible();
  await drawer.getByRole('button', { name: 'Set hold' }).click();
  await expect(drawer.getByRole('status')).toHaveText('Hold set.');
  await expect(page).toHaveURL(before);
});

test('work details fit a 390px viewport without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route('**/api/work/**', (route) => route.fulfill({ json: [] }));
  await page.goto('/');
  await page.getByRole('button', { name: 'Open navigation menu' }).click();
  await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Factory', exact: true }).click();
  await page.getByRole('button', { name: 'View details', exact: true }).first().click();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
