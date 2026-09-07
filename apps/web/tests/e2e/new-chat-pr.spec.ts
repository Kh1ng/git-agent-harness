import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

// Coverage for issue #1033: the New Chat modal's PR tab lists the project's
// open PRs (author, draft/review state) and starts a read-only chat seeded
// with the PR — no worktree, nothing at the provider mutated. Empty results stay distinct from recoverable provider failures.

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
  await page.route('**/api/manager-chat/prs**', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ prs: [] }) })
      : route.continue());
  await page.getByRole('button', { name: 'Start chat' }).click();

  // The modal closes and the fresh session is selected, its transcript
  // seeded with the PR.
  await expect(page.getByRole('dialog', { name: 'New chat' })).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /#12 Ship the PR chat mode/ })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('Head branch: feat/pr-chat')).toBeVisible();
  await expect(page.getByRole('link', { name: 'View PR' })).toHaveAttribute('href', 'https://github.com/Kh1ng/git-agent-harness/pull/12');
  await expect(page.getByRole('button', { name: 'Commit', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Refresh git data' })).toBeEnabled();

  // The created session is read-only on the provider: worktree-less.
  const state = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
  expect(state.ok(), await state.text()).toBe(true);
  const { sessions } = await state.json() as { sessions: { title: string; worktreePath: string | null; branch: string; prNumber?: number }[] };
  const created = sessions.find((session) => session.title === '#12 Ship the PR chat mode');
  expect(created, 'the PR session exists in the mock').toBeTruthy();
  expect(created!.worktreePath).toBeNull();
  expect(created!.branch).toBe('feat/pr-chat');
  expect(created!.prNumber).toBe(12);
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

  // Provider failures retain their reason and offer retry instead of claiming emptiness.
  for (const source of [
    { tab: 'From PR', path: 'prs', label: 'pull requests', empty: 'No open pull requests for this project.', number: 22 },
    { tab: 'From issue', path: 'issues', label: 'issues', empty: 'No open issues for this project.', number: 23 }
  ]) {
    let attempts = 0;
    let failing = true;
    await page.route(`**/api/manager-chat/${source.path}**`, (route) => {
      attempts++;
      return failing
        ? route.fulfill({ status: 503, json: { message: 'Provider temporarily unavailable' } })
        : route.fulfill({ json: { [source.path]: [{ number: source.number, title: 'Recovered provider item', labels: [], author: 'octocat', isDraft: false, reviewState: null }] } });
    });
    await openNewChatModal(page);
    const dialog = page.getByRole('dialog', { name: 'New chat' });
    await dialog.getByRole('tab', { name: source.tab }).click();
    await expect(dialog.getByRole('alert')).toContainText(`Could not load ${source.label}. Provider temporarily unavailable`);
    await expect(dialog.getByText(source.empty)).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Start chat' })).toBeDisabled();
    await dialog.getByRole('searchbox').fill('Recovered');
    const attemptsBeforeRetry = attempts;
    failing = false;
    await dialog.getByRole('button', { name: `Retry ${source.label}` }).click();
    await expect(dialog.getByRole('button', { name: `#${source.number} Recovered provider item`, exact: false })).toBeVisible();
    await expect(dialog.getByRole('alert')).toHaveCount(0);
    await expect(dialog.getByRole('searchbox')).toHaveValue('Recovered');
    expect(attempts).toBe(attemptsBeforeRetry + 1);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  }
});

test('git strip commits a writable checkout and exposes an accessible refresh action', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  let message: string | undefined;
  await page.route('**/api/git/commit?**', async (route) => {
    message = (await route.request().postDataJSON() as { message?: string }).message;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ hash: 'abcdef1' }) });
  });
  await openChat(page);

  await expect(page.getByRole('button', { name: 'Commit', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Commit', exact: true }).click();
  const commitMessage = page.getByPlaceholder('Commit message');
  await commitMessage.fill('fix the chat git actions');
  await commitMessage.locator('..').getByRole('button', { name: 'Commit', exact: true }).click();

  await expect(commitMessage).toHaveCount(0);
  expect(message).toBe('fix the chat git actions');
  await expect(page.getByRole('button', { name: 'Refresh git data' })).toBeEnabled();
});
