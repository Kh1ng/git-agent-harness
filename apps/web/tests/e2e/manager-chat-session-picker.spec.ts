import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

async function selectScenario(request: APIRequestContext, name: string): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

/** Chat opens a blank conversation; every existing project and chat lives
 * on the Projects page (#1199). */
async function openProjects(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Projects', exact: true }).click();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
}

test('project and chat navigation stays visible without a global dropdown', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openProjects(page);

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
  await openProjects(page);

  const rail = page.getByRole('complementary', { name: 'Chat navigation' });
  const projects = rail.getByRole('navigation', { name: 'Projects' });
  await rail.locator('summary').filter({ hasText: 'Projects' }).click();
  await expect(projects).not.toBeVisible();
  await expect(rail.getByRole('navigation', { name: 'Chats', exact: true })).toBeVisible();
});

test('selecting a chat opens it on the chat page with its own provider', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openProjects(page);

  await page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ }).click();

  // The chat page takes over: no rail, the session's own provider selection.
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex', { timeout: 10_000 });
  await expect(page).toHaveURL(/[?&]page=chat/);
  await expect(page).toHaveURL(/[?&]chat=/);

  // And the way back to everything else is one button, not a second rail.
  await page.getByRole('button', { name: 'Projects', exact: true }).first().click();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
});

test('the open issues queue starts a seeded chat in one step', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openProjects(page);

  const issues = page.getByRole('region', { name: 'Open issues' });
  const first = issues.getByRole('button', { name: 'Start chat', exact: true }).first();
  await expect(first).toBeEnabled({ timeout: 10_000 });
  await first.click();

  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible({ timeout: 10_000 });
  await expect(page).toHaveURL(/[?&]page=chat/);
});
