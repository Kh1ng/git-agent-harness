import { expect, test } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

test('edits a published pull request through the mock control plane', async ({ page, request }) => {
  await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name: 'normal' } });
  await page.goto('/?page=git&profile=fixture');
  await page.getByRole('button', { name: 'Pull Requests' }).click();
  const row = page.getByRole('row').filter({ hasText: 'Ship the PR chat mode' });
  await row.click();
  const panel = page.getByRole('dialog', { name: 'Commit and pull request review' });
  await panel.getByRole('button', { name: 'Continue with uncommitted changes' }).click();
  await panel.getByLabel('Pull request title').fill('Published title edited in dashboard');
  await panel.getByLabel('Pull request body').fill('Published description edited in dashboard');
  page.once('dialog', dialog => dialog.accept());
  await panel.getByRole('button', { name: 'Update published title and description' }).click();
  await expect(panel.getByRole('link', { name: 'Open pull request' }).first()).toBeVisible();

  const response = await request.get(`${MOCK_BASE_URL}/api/git/prs`);
  expect(response.ok(), await response.text()).toBe(true);
  const body = await response.json() as { prs: { number: number; title: string }[] };
  expect(body.prs.find(pr => pr.number === 12)?.title).toBe('Published title edited in dashboard');
});
