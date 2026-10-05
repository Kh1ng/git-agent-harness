import { expect, test } from '@playwright/test';

// The Git issues icon on the left strip lists the project's open issues;
// an issue opens the same work detail drawer Needs attention uses.

// Routes that fetch from the fixture server must not outlive the test.
test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test('the Git issues sidebar lists open issues and opens the work drawer', async ({ page }) => {
  await page.route('**/api/manager-chat/issues**', (route) => route.fulfill({ json: { issues: [
    { number: 946, title: 'Retry loop never stops', url: 'https://github.com/Kh1ng/git-agent-harness/issues/946', labels: ['bug'], updatedAt: '2026-10-04T20:00:00Z' },
    { number: 950, title: 'Add caching to the router', url: null, labels: [], updatedAt: '2026-10-04T22:00:00Z' }
  ] } }));
  await page.goto('/?page=overview&profile=fixture');
  await page.getByRole('navigation', { name: 'Sidebar' }).getByRole('button', { name: 'Git issues', exact: true }).click();
  const panel = page.getByRole('complementary', { name: 'Git issues' });
  await expect(panel.getByRole('heading', { name: 'Git issues', exact: true })).toBeVisible();
  expect(new URL(page.url()).searchParams.get('side')).toBe('issues');
  const issues = panel.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
  await expect(issues).toHaveCount(2);
  // Newest update first.
  await expect(issues.first()).toContainText('#950');
  await expect(issues.nth(1)).toContainText('bug · updated');
  await expect(issues.nth(1).getByRole('link', { name: 'Open #946 on the provider' })).toHaveAttribute('href', 'https://github.com/Kh1ng/git-agent-harness/issues/946');
  await panel.getByRole('searchbox', { name: 'Filter issues' }).fill('retry');
  await expect(issues).toHaveCount(1);
  // The detail expands inside the sidebar, not over the page; Back returns to the list.
  await issues.first().getByRole('button').first().click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Back to the list' })).toBeVisible();
  await expect(panel).toContainText('#946');
  await expect(panel.getByRole('heading', { name: 'Attempt history' })).toBeVisible();
  await panel.getByRole('button', { name: 'Back to the list' }).click();
  await expect(panel.getByRole('heading', { name: 'Git issues', exact: true })).toBeVisible();
});

test('Start chat on an issue switches a chat that is already docked to the new conversation', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/?page=overview&profile=fixture&dock=chat&side=issues');
  const dock = page.getByRole('complementary', { name: 'Chat', exact: true });
  const picker = dock.getByRole('combobox', { name: 'Chat', exact: true });
  await expect(picker).toHaveValue('');
  const panel = page.getByRole('complementary', { name: 'Git issues' });
  await panel.getByRole('button', { name: 'Start a chat on #1087' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('chat')).toMatch(/.+/);
  const chat = new URL(page.url()).searchParams.get('chat')!;
  await expect(picker).toHaveValue(chat);
  await expect(picker.locator('option:checked')).toContainText('#1087');
});
