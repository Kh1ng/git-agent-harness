import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

// Coverage for issue #1033: the New Chat modal's PR tab lists the project's
// open PRs (author, draft/review state) and starts a read-only chat seeded
// with the PR — no worktree, nothing at the provider mutated. Empty and
// failed states mirror the issue tab.

async function selectScenario(request: APIRequestContext, name: string): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

async function openChat(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

async function openNewChatModal(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'New chat' }).click();
  await expect(page.getByRole('dialog', { name: 'New chat' })).toBeVisible();
}

test('PR tab lists open PRs and starts a chat seeded with one', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await openNewChatModal(page);

  // The mock reports no issues but two open PRs for the fixture project;
  // the PR tab renders author + draft/review state like the issue tab.
  await page.getByRole('tab', { name: 'From PR' }).click();
  await expect(page.getByText('#12 Ship the PR chat mode')).toBeVisible();
  await expect(page.getByText('octocat · approved')).toBeVisible();
  await expect(page.getByText('hubot · draft · review required')).toBeVisible();

  await page.getByText('#12 Ship the PR chat mode').click();
  await expect(page.getByText(/read-only: no branch is created and the PR is not modified/)).toBeVisible();
  await page.getByRole('button', { name: 'Start chat' }).click();

  // The modal closes and the fresh session is selected, its transcript
  // seeded with the PR.
  await expect(page.getByRole('dialog', { name: 'New chat' })).toHaveCount(0);
  await expect(page.getByLabel('Chat session').locator('option', { hasText: '#12 Ship the PR chat mode' })).toHaveCount(1);
  await expect(page.getByLabel('Chat session')).not.toHaveValue('');
  await expect(page.getByText('Head branch: feat/pr-chat')).toBeVisible();

  // The created session is read-only on the provider: worktree-less.
  const state = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
  expect(state.ok(), await state.text()).toBe(true);
  const { sessions } = await state.json() as { sessions: { title: string; worktreePath: string | null; branch: string }[] };
  const created = sessions.find((session) => session.title === '#12 Ship the PR chat mode');
  expect(created, 'the PR session exists in the mock').toBeTruthy();
  expect(created!.worktreePath).toBeNull();
  expect(created!.branch).toBe('feat/pr-chat');
});

test('PR tab empty and failed states match the issue tab', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  // No open PRs: same empty state as the issue tab, Start disabled.
  await page.route('**/api/manager-chat/prs**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ prs: [] }) }));
  await openNewChatModal(page);
  await page.getByRole('tab', { name: 'From PR' }).click();
  await expect(page.getByText('No open pull requests for this project.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start chat' })).toBeDisabled();
  await page.getByRole('button', { name: 'Close' }).click();

  // A failed PR list degrades to the empty state, exactly like issues.
  await page.route('**/api/manager-chat/prs**', (route) =>
    route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'Failed to list pull requests' }) }));
  await openNewChatModal(page);
  await page.getByRole('tab', { name: 'From PR' }).click();
  await expect(page.getByText('No open pull requests for this project.')).toBeVisible();

  // The issue tab is still its old self alongside the new tab.
  await page.getByRole('tab', { name: 'From issue' }).click();
  await expect(page.getByText('No open issues for this project.')).toBeVisible();
});
