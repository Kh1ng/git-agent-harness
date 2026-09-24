import { expect, test } from '@playwright/test';

test('switches the Git surface between configured projects', async ({ page }) => {
  await page.addInitScript(() => {
    window.__GAH_DESKTOP_OPEN_PROJECT__ = true;
    (window as unknown as { openCalls: unknown[] }).openCalls = [];
    (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__ = {
      invoke: async (command, args) => {
        (window as unknown as { openCalls: unknown[] }).openCalls.push({ command, args });
        return { available: false, preferredTool: null, tools: [], reason: 'This checkout belongs to another device.' };
      }
    };
  });
  const profiles = [
    { name: 'fixture', display_name: 'Fixture', provider: 'github', repo: 'org/fixture', local_path: '/tmp/fixture', web_url: null, max_parallel_workers: null, max_open_managed_mrs: 1, validation_timeout_seconds: 300, chat_session_idle_days: 14, manager_wake_autonomy: 'off', delivery_mode: 'pr', repo_id: 'fixture', worktree_base: '/tmp/worktrees' },
    { name: 'second', display_name: 'Second project', provider: 'github', repo: 'org/second', local_path: '/tmp/second', web_url: null, max_parallel_workers: null, max_open_managed_mrs: 1, validation_timeout_seconds: 300, chat_session_idle_days: 14, manager_wake_autonomy: 'off', delivery_mode: 'pr', repo_id: 'second', worktree_base: '/tmp/worktrees' }
  ];
  await page.route('**/api/profiles', (route) => route.fulfill({ json: profiles }));
  await page.route('**/api/git/status**', (route) => {
    const profile = new URL(route.request().url()).searchParams.get('profile');
    return route.fulfill({ json: { branch: profile === 'second' ? 'main-second' : 'main-fixture', changes: [], cwd: `/tmp/${profile}`, ownerNodeId: 'central-1', ownerNodeName: 'MacBook' } });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Git', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Project' })).toHaveValue('fixture');

  await page.getByRole('combobox', { name: 'Project' }).selectOption('second');
  await expect(page.getByText('main-second', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Files on MacBook')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { openCalls: unknown[] }).openCalls)).toContainEqual({
    command: 'desktop_open_context',
    args: { project: { profile: 'second', nodeId: 'central-1' } }
  });
});
