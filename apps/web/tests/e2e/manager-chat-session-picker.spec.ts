import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

async function selectScenario(request: APIRequestContext, name: string): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

async function openChat(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

test('project and chat navigation stays visible without a global dropdown', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const rail = page.getByRole('complementary', { name: 'Chat navigation' });
  await expect(rail.getByRole('navigation', { name: 'Projects' }).getByRole('button', { name: /Fixture/ })).toBeVisible();
  await expect(rail.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: 'Default conversation' })).toBeVisible();
  await expect(rail.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ })).toBeVisible();
  await expect(page.getByRole('listbox', { name: 'All sessions' })).toHaveCount(0);

  await expect(rail.getByText('Archived (1)')).toBeVisible();
  await expect(rail.getByRole('navigation', { name: 'Archived chats' })).not.toBeVisible();
  await rail.getByText('Archived (1)').click();
  await expect(rail.getByRole('navigation', { name: 'Archived chats' }).getByRole('button', { name: /Settled mock session/ })).toBeVisible();
});

test('projects collapse independently while chats stay usable', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const rail = page.getByRole('complementary', { name: 'Chat navigation' });
  const projects = rail.getByRole('navigation', { name: 'Projects' });
  await rail.locator('summary').filter({ hasText: 'Projects' }).click();
  await expect(projects).not.toBeVisible();
  await expect(rail.getByRole('navigation', { name: 'Chats', exact: true })).toBeVisible();
});

test('selecting a chat opens it without hiding navigation', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const chats = page.getByRole('navigation', { name: 'Chats', exact: true });
  const session = chats.getByRole('button', { name: /Mock session/ });
  await session.click();

  await expect(session).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex', { timeout: 10_000 });
  await expect(chats).toBeVisible();
});
