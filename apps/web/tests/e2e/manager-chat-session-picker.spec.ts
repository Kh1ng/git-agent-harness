import { expect, test, type APIRequestContext } from '@playwright/test';
import { openProjects } from './helpers/navigation.js';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

async function selectScenario(request: APIRequestContext, name: string): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
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

test('selecting a chat keeps the project rail and opens its own provider', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openProjects(page);

  await page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ }).click();

  // Chat keeps the existing rail and uses the session's own provider selection.
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex', { timeout: 10_000 });
  await expect(page).toHaveURL(/[?&]page=chat/);
  await expect(page).toHaveURL(/[?&]chat=mock-session-1/);
  // Docking keeps the conversation; expanding again brings the rail back.
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click();
  await expect(page).toHaveURL(/[?&]dock=chat/);
  await expect(page).toHaveURL(/[?&]chat=mock-session-1/);
  await page.getByRole('button', { name: 'Expand chat' }).click();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
});

test('the Chat action opens no launcher; New chat is a panel inside the chat', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();

  // Expanded, the Chat button docks the chat back beside the page it covered.
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click();
  await expect(page).toHaveURL(/[?&]dock=chat/);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'New chat' })).toHaveCount(0);

  await page.getByRole('button', { name: 'New chat', exact: true }).click();
  const panel = page.getByRole('region', { name: 'New chat' });
  await expect(panel.getByRole('button', { name: 'Start chat' })).toBeDisabled();
  // Project, node and provider wait behind Extra settings; the summary names the current choice.
  await expect(panel.getByRole('combobox', { name: 'Run on node' })).toBeHidden();
  await expect(panel.getByText('Extra settings')).toBeVisible();
  await panel.getByText('Extra settings').click();
  await expect(panel.getByRole('button', { name: /Fixture/ })).toBeVisible();
  await expect(panel.getByRole('combobox', { name: 'Run on node' })).toBeVisible();
  await panel.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(panel).toHaveCount(0);
});

test('the Git issues sidebar starts a seeded chat in one step', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await page.goto('/?page=overview&profile=fixture');
  await page.getByRole('navigation', { name: 'Sidebar' }).getByRole('button', { name: 'Git issues', exact: true }).click();
  // The mock's own open issue; starting a chat on it is validated server side.
  const start = page.getByRole('button', { name: 'Start a chat on #1087' });
  await expect(start).toBeEnabled({ timeout: 10_000 });
  await start.click();

  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible({ timeout: 10_000 });
  await expect(page).toHaveURL(/[?&](page|dock)=chat/);
  await expect(page).toHaveURL(/[?&]chat=mock-session-/);
});
