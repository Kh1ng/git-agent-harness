import { expect, test } from '@playwright/test';

test('edits and publishes a review in-page without navigation', async ({ page }) => {
  let published: Record<string, unknown> | null = null;
  await page.route('**/api/git/review**', route => route.fulfill({ json: {
    ownerNodeId: 'local', ownerNodeName: 'Fixture node', provider: 'github', providerLabel: 'pull request', branch: 'feat/pr-chat', base: 'main', upstream: 'origin/feat/pr-chat',
    ahead: 1, behind: 0, files: [], commits: [{ hash: 'abcdef1', short: 'abcdef1', subject: 'Ship the PR chat mode' }], changedFiles: ['README.md'], patch: 'diff',
    existing: { number: 12, title: 'Ship the PR chat mode', url: 'https://github.com/Kh1ng/git-agent-harness/pull/12', draft: false },
  } }));
  await page.route('**/api/git/publish**', async route => {
    published = route.request().postDataJSON();
    await route.fulfill({ json: { url: 'https://github.com/Kh1ng/git-agent-harness/pull/12', existing: true } });
  });
  await page.goto('/?page=git&profile=fixture');
  await page.getByRole('button', { name: 'Pull Requests' }).click();
  await page.getByRole('row').filter({ hasText: 'Ship the PR chat mode' }).click();
  const panel = page.getByRole('dialog', { name: 'Commit and pull request review' });
  await expect(panel).toBeVisible();
  const before = page.url();
  await panel.getByLabel('Pull request title').fill('A clearer title');
  await panel.getByLabel('Pull request body').fill('A clearer description');
  page.once('dialog', dialog => dialog.accept());
  await panel.getByRole('button', { name: 'Push and update pull request' }).click();
  await expect(panel.getByRole('link', { name: 'Open pull request' }).first()).toHaveAttribute('href', 'https://github.com/Kh1ng/git-agent-harness/pull/12');
  expect(published).toMatchObject({ title: 'A clearer title', body: 'A clearer description', base: 'main', draft: false });
  await expect(page).toHaveURL(before);
});
