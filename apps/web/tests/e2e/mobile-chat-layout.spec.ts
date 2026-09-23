import { expect, test } from '@playwright/test';

for (const width of [320, 390]) {
  test(`mobile chat at ${width}px keeps the conversation ahead of navigation and secondary tools`, async ({ page, request }, testInfo) => {
    const mock = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';
    expect((await request.post(`${mock}/api/mock/scenario`, { data: { name: 'normal' } })).ok()).toBe(true);
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/?page=chat&profile=fixture&chat=mock-session-1');
    const projectSelector = page.getByRole('combobox', { name: 'Project' });
    const chatSelector = page.getByRole('combobox', { name: 'Chat' });
    const rail = page.getByRole('complementary', { name: 'Chat navigation' });
    const draft = page.getByPlaceholder(/Message the manager/);
    const providerPicker = page.getByRole('button', { name: 'Provider picker' });
    await expect(providerPicker).toBeVisible();
    await providerPicker.click();
    const providerDialog = page.getByRole('dialog', { name: 'Provider picker' });
    await expect(providerDialog).toBeVisible();
    expect(await providerDialog.evaluate((dialog) => {
      const bounds = dialog.getBoundingClientRect();
      return bounds.left >= 0 && bounds.right <= window.innerWidth
        && bounds.top >= 0 && bounds.bottom <= window.innerHeight;
    })).toBe(true);
    for (const control of await providerDialog.locator('button:visible').all()) {
      const bounds = (await control.boundingBox())!;
      expect(bounds.height).toBeGreaterThanOrEqual(44);
      expect(bounds.width).toBeGreaterThanOrEqual(44);
    }
    await page.screenshot({ path: testInfo.outputPath(`provider-picker-${width}.png`), fullPage: true });
    await providerDialog.getByRole('button', { name: 'Close provider picker' }).click();
    await expect(page.getByRole('button', { name: /1 changed file\./ })).toBeVisible();
    await expect(projectSelector).toHaveValue('fixture');
    await expect(chatSelector).toHaveValue('mock-session-1');
    await expect(chatSelector.locator('optgroup[label^="Archived"] option')).toHaveCount(1);
    await expect(page.getByRole('button', { name: /Projects & chats/ })).toHaveCount(0);
    await expect(rail).toBeHidden();
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeHidden();
    await expect(page.getByRole('button', { name: 'New chat', exact: true })).toBeVisible();
    await draft.fill('Keep this unfinished message');
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
    await page.getByRole('button', { name: 'Close chat tools' }).click();
    await page.getByLabel('Skills', { exact: true }).click();
    const skills = page.getByText('Skills', { exact: true });
    await expect(skills).toBeVisible();
    const skillsBounds = await skills.boundingBox();
    expect(skillsBounds!.x).toBeGreaterThanOrEqual(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`chat-tools-${width}.png`), fullPage: true });
    await page.getByLabel('Skills', { exact: true }).click();
    await expect(tools).toHaveAttribute('aria-expanded', 'false');
    await expect(draft).toHaveValue('Keep this unfinished message');
    await expect(page.getByLabel('Run on node', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
    for (const control of [
      page.getByRole('button', { name: 'Provider picker' }),
      page.getByLabel('Run on node', { exact: true }),
      page.getByLabel('Skills', { exact: true }),
      page.getByRole('button', { name: 'Send', exact: true })
    ]) {
      const bounds = (await control.boundingBox())!;
      expect(bounds.height).toBeGreaterThanOrEqual(44);
      expect(bounds.width).toBeGreaterThanOrEqual(44);
    }
    for (const selector of [projectSelector, chatSelector]) {
      const target = await selector.boundingBox();
      expect(target!.height).toBeGreaterThanOrEqual(44);
    }
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: testInfo.outputPath(`chat-${width}.png`), fullPage: true });

    if (width === 390) {
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      const stop = page.getByRole('button', { name: 'Stop', exact: true });
      await expect(stop).toBeVisible();
      const controls = [
        page.getByRole('button', { name: 'Provider picker' }),
        page.getByLabel('Run on node', { exact: true }),
        page.getByLabel('Skills', { exact: true }),
        stop,
        page.getByRole('button', { name: 'Send', exact: true })
      ];
      const bounds = await Promise.all(controls.map((control) => control.boundingBox()));
      expect(Math.max(...bounds.map((box) => box!.y))).toBeLessThan(
        Math.min(...bounds.map((box) => box!.y + box!.height))
      );
      await expect(stop).toHaveCount(0);
    }

    await chatSelector.selectOption('');
    await expect.poll(() => new URL(page.url()).searchParams.get('chat')).toBeNull();
    await expect(chatSelector).toHaveValue('');
    await expect(rail).toBeHidden();
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(rail).toBeVisible();
    await expect(projectSelector).toBeHidden();
    const desktopSend = (await page.getByRole('button', { name: 'Send', exact: true }).boundingBox())!;
    expect(desktopSend.width).toBeGreaterThanOrEqual(34);
    expect(desktopSend.height).toBeGreaterThanOrEqual(36);
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeHidden();
    await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Storage', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('chat-desktop.png'), fullPage: true });
  });
}

test('chat keeps the composer inside short and offline viewports', async ({ page, request }) => {
  const mock = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';
  expect((await request.post(`${mock}/api/mock/scenario`, { data: { name: 'normal' } })).ok()).toBe(true);
  await page.setViewportSize({ width: 667, height: 375 });
  await page.goto('/?page=chat&profile=fixture&chat=mock-session-1');
  const draft = page.getByPlaceholder(/Message the manager/);
  await expect(draft).toBeVisible();

  const expectComposerInsideViewport = async () => {
    await expect.poll(() => draft.evaluate((textarea) => {
      const composer = textarea.parentElement!.getBoundingClientRect();
      return document.documentElement.scrollHeight <= window.innerHeight && composer.bottom <= window.innerHeight;
    })).toBe(true);
  };

  await expectComposerInsideViewport();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.context().setOffline(true);
  await expect(page.getByText(/^Offline/)).toBeVisible();
  await expectComposerInsideViewport();
  await page.setViewportSize({ width: 1440, height: 900 });
  await expectComposerInsideViewport();
  await page.setViewportSize({ width: 1024, height: 400 });
  await expectComposerInsideViewport();
  const primaryNavigation = page.getByRole('navigation', { name: 'Primary' });
  const sidebar = primaryNavigation.locator('..');
  await expect(primaryNavigation).toBeVisible();
  expect(await sidebar.evaluate((element) => element.scrollHeight > element.clientHeight
    && getComputedStyle(element).overflowY === 'auto')).toBe(true);
  const settings = primaryNavigation.getByRole('button', { name: 'Settings', exact: true });
  await settings.scrollIntoViewIfNeeded();
  await expect(settings).toBeInViewport();
});
