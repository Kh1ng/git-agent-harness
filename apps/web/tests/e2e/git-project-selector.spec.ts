import { expect, test } from '@playwright/test';

test('switches the Git surface between configured projects', async ({ page }) => {
  const profiles = [
    { name: 'fixture', display_name: 'Fixture', provider: 'github', repo: 'org/fixture', local_path: '/tmp/fixture', web_url: null, max_parallel_workers: null, max_open_managed_mrs: 1, validation_timeout_seconds: 300, chat_session_idle_days: 14, manager_wake_autonomy: 'off', delivery_mode: 'pr', repo_id: 'fixture', worktree_base: '/tmp/worktrees' },
    { name: 'second', display_name: 'Second project', provider: 'github', repo: 'org/second', local_path: '/tmp/second', web_url: null, max_parallel_workers: null, max_open_managed_mrs: 1, validation_timeout_seconds: 300, chat_session_idle_days: 14, manager_wake_autonomy: 'off', delivery_mode: 'pr', repo_id: 'second', worktree_base: '/tmp/worktrees' }
  ];
  await page.route('**/api/profiles', (route) => route.fulfill({ json: profiles }));
  await page.route('**/api/git/status**', (route) => {
    const profile = new URL(route.request().url()).searchParams.get('profile');
    return route.fulfill({ json: { branch: profile === 'second' ? 'main-second' : 'main-fixture', changes: [], cwd: `/tmp/${profile}` } });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Git', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Project' })).toHaveValue('fixture');

  await page.getByRole('combobox', { name: 'Project' }).selectOption('second');
  await expect(page.getByText('main-second', { exact: true }).first()).toBeVisible();
});
