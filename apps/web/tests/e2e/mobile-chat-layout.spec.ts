import { expect, test } from '@playwright/test';

for (const width of [320, 390]) {
  test(`mobile chat at ${width}px keeps the conversation ahead of navigation and secondary tools`, async ({ page, request }, testInfo) => {
    const mock = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';
    expect((await request.post(`${mock}/api/mock/scenario`, { data: { name: 'normal' } })).ok()).toBe(true);
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/?page=chat&profile=fixture&chat=mock-session-1');
    const navigator = page.getByRole('button', { name: /Projects & chats/ });
    const rail = page.getByRole('complementary', { name: 'Chat navigation' });
    const draft = page.getByPlaceholder(/Message the manager/);
    await expect(page.getByRole('button', { name: 'Provider picker' })).toBeVisible();
    await expect(navigator).toContainText('Mock session');
    await expect(rail).toBeHidden();
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeHidden();
    await expect(page.getByRole('button', { name: 'New chat', exact: true })).toBeVisible();
    await draft.fill('Keep this unfinished message');
    await navigator.click();
    await expect(rail).toBeVisible();
    const filter = rail.getByRole('searchbox');
    await filter.fill('mock');
    await navigator.click();
    await expect(draft).toHaveValue('Keep this unfinished message');
    await navigator.click();
    await expect(filter).toHaveValue('mock');
    await filter.clear();
    await rail.getByRole('button', { name: /Mock session/ }).click();
    await expect(navigator).toBeFocused();
    await expect(navigator).toHaveAttribute('aria-expanded', 'false');
    await expect(draft).toHaveValue('Keep this unfinished message');

    const tools = page.getByRole('button', { name: 'Chat tools', exact: true });
    await tools.click();
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Archive', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Refresh git data' })).toBeVisible();
    for (const control of await page.locator('#chat-tools button:visible, #chat-tools summary:visible, #chat-git button:visible').all()) {
      const bounds = (await control.boundingBox())!;
      expect(bounds.height).toBeGreaterThanOrEqual(44);
      expect(bounds.width).toBeGreaterThanOrEqual(44);
    }
    await page.getByRole('button', { name: 'Commit', exact: true }).click();
    await expect(page.getByPlaceholder('Commit message')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.getByPlaceholder('Commit message').press('Escape');
    await page.getByLabel('Project skills', { exact: true }).click();
    const skills = page.getByText('Project skills', { exact: true });
    await expect(skills).toBeVisible();
    const skillsBounds = await skills.boundingBox();
    expect(skillsBounds!.x).toBeGreaterThanOrEqual(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`chat-tools-${width}.png`), fullPage: true });
    await page.getByLabel('Project skills', { exact: true }).click();
    await tools.click();
    await expect(draft).toHaveValue('Keep this unfinished message');
    await expect(page.getByLabel('Run on node', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
    const target = await navigator.boundingBox();
    expect(target!.height).toBeGreaterThanOrEqual(44);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: testInfo.outputPath(`chat-${width}.png`), fullPage: true });

    await navigator.click();
    await rail.getByRole('button', { name: 'Default conversation', exact: true }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('chat')).toBeNull();
    await expect(navigator).toContainText('Default conversation');
    await expect(rail).toBeHidden();
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(rail).toBeVisible();
    await expect(navigator).toBeHidden();
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('chat-desktop.png'), fullPage: true });
  });
}
