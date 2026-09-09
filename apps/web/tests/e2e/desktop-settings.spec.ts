import { expect, test } from '@playwright/test';

test('desktop Settings keeps local controls reachable when central requests fail', async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, '__GAH_DESKTOP_SETTINGS__', { value: true });
  });
  await page.route(url => url.pathname.startsWith('/api/'), route => route.fulfill({ status: 503, json: { message: 'Central unavailable' } }));
  await page.goto('/?page=settings');
  const localSettings = page.getByRole('link', { name: 'This computer', exact: true });
  await expect(localSettings).toBeVisible();
  await expect(localSettings).toHaveAttribute('href', 'gah://settings');
  await expect(localSettings).not.toHaveAttribute('target', '_blank');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 844 });
    await localSettings.focus();
    await expect(localSettings).toBeFocused();
    const box = await localSettings.boundingBox();
    expect(box?.height).toBeGreaterThanOrEqual(44);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`desktop-settings-${width}.png`) });
  }
});

test('ordinary browser Settings omits desktop-only navigation at desktop and mobile widths', async ({ page }) => {
  await page.goto('/?page=settings');
  await expect(page.getByRole('heading', { name: 'Connection & pairing' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'This computer', exact: true })).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Connection & pairing' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'This computer', exact: true })).toHaveCount(0);
});
