import { expect, test } from '@playwright/test';

test('mobile pages keep connection setup in Settings', async ({ page }, testInfo) => {
  for (const width of [320, 390]) {
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/?page=overview');
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Device pairing' })).toHaveCount(0);
    await expect(page.getByText('Central access token', { exact: true })).toHaveCount(0);
    await expect(page.locator('header').getByTestId('frontend-build')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`overview-${width}.png`) });
    await page.getByRole('button', { name: 'Open navigation menu' }).click();
    await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Connection & pairing' })).toBeVisible();
    await expect(page.getByText('Central access token', { exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Device pairing' })).toBeVisible();
    await expect(page.getByTestId('settings-build')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`settings-${width}.png`) });
  }
});
