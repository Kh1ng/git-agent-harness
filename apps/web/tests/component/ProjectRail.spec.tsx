import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { ProjectSummary } from '@git-agent-harness/contracts';
import { ProjectRail } from '../../src/components/ProjectRail.js';

const local: ProjectSummary = {
  name: 'repo', display_name: 'Example project', provider: 'github', repo: 'team/repo', repo_id: 'repo',
  local_path: '/central/repo', worktree_base: '/central/worktrees', web_url: 'https://github.com/team/repo',
  max_parallel_workers: null, max_open_managed_mrs: 1, manager_wake_autonomy: null, validation_timeout_seconds: 300,
  node_id: 'central', chat_profile: 'repo'
};
const remote: ProjectSummary = { ...local, node_id: 'worker', chat_profile: 'gah-node:worker:repo', local_path: '/worker/repo' };

test('groups equal project names by node, selects the remote conversation, and imports with explicit GitLab settings', async ({ mount, page }, testInfo) => {
  await page.route('**/api/manager-chat/nodes', (route) => route.fulfill({ json: { nodes: [
    { nodeId: 'central', displayName: 'Control computer', role: 'central', chatCapable: true, lastSeenAt: null },
    { nodeId: 'worker', displayName: 'Windows workstation', role: 'worker', chatCapable: true, lastSeenAt: null }
  ] } }));
  const selected: (string | null)[] = [];
  const added: ProjectSummary[] = [];
  let request: Record<string, unknown> | undefined;
  await page.route('**/api/projects/import', (route) => {
    request = route.request().postDataJSON();
    return route.fulfill({ json: { project: remote, checkoutPath: remote.local_path, checkoutStatus: 'cloned', detectedLanguages: ['Rust'], validationCommands: ['cargo test'] } });
  });
  const component = await mount(<div className="w-full xl:max-w-64"><ProjectRail profiles={[local, remote]} currentProfile={remote.chat_profile}
    sessions={[]} selectedSessionId={null} onSelect={(profile) => selected.push(profile)} onSessionSelect={() => {}}
    sessionsError={false} onRetrySessions={() => {}} onProjectAdded={(project) => added.push(project)} /></div>);
  const remoteGroup = component.getByRole('region', { name: 'Projects on Windows workstation' });
  await expect(remoteGroup.getByText('Runs on Windows workstation')).toBeVisible();
  await expect(remoteGroup.getByRole('button')).toHaveAttribute('aria-current', 'page');
  await remoteGroup.getByRole('button').click();
  expect(selected).toEqual([remote.chat_profile]);
  await component.getByText('Import from Git', { exact: true }).click();
  await component.getByLabel('Import on').selectOption('worker');
  await component.getByLabel('Git repository URL').fill('ssh://git@git.example.test:2222/team/repo.git');
  await component.getByLabel('Repository provider').selectOption('gitlab');
  await expect(component.getByRole('button', { name: 'Import repository', exact: true })).toBeDisabled();
  await component.getByLabel('GitLab project ID', { exact: true }).fill('123');
  await component.getByLabel('GitLab API URL (optional)').fill('https://git.example.test:8443/api/v4');
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: testInfo.outputPath('project-import-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 1100 });
  await page.screenshot({ path: testInfo.outputPath('project-import-mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await component.getByRole('button', { name: 'Import repository', exact: true }).click();
  await expect(component.getByRole('status').filter({ hasText: 'cloned team/repo' })).toBeVisible();
  expect(request).toEqual({ gitUrl: 'ssh://git@git.example.test:2222/team/repo.git', nodeId: 'worker', reclone: false, provider: 'gitlab', providerProjectId: '123', providerApiBase: 'https://git.example.test:8443/api/v4' });
  expect(added[0].node_id).toBe('worker');
  expect(selected.at(-1)).toBe(remote.chat_profile);
});

test('failed worker import explains the failure and preserves the current project', async ({ mount, page }) => {
  await page.route('**/api/manager-chat/nodes', (route) => route.fulfill({ json: { nodes: [] } }));
  await page.route('**/api/projects/import', (route) => route.fulfill({ status: 502, json: { message: 'The selected worker is offline or unreachable. No project was imported.' } }));
  const selected: (string | null)[] = [];
  const component = await mount(<ProjectRail profiles={[local]} currentProfile="repo" sessions={[]} selectedSessionId={null}
    onSelect={(profile) => selected.push(profile)} onSessionSelect={() => {}} sessionsError={false} onRetrySessions={() => {}} onProjectAdded={() => {}} />);
  await component.getByText('Import from Git', { exact: true }).click();
  await component.getByLabel('Git repository URL').fill('https://github.com/team/repo');
  await component.getByRole('button', { name: 'Import repository', exact: true }).click();
  await expect(component.getByRole('alert')).toContainText('offline or unreachable');
  expect(selected).toEqual([]);
  await expect(component.getByLabel('Git repository URL')).toHaveValue('https://github.com/team/repo');
});
