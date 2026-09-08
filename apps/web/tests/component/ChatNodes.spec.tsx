import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { NewChatModal, type ChatProfile } from '../../src/components/NewChatModal.js';

const local: ChatProfile = {
  name: 'gah', display_name: 'Local project', repo: 'owner/gah', provider: 'github', local_path: '/tmp/gah',
  repo_id: 'gah', worktree_base: '/tmp/worktrees', web_url: null, max_parallel_workers: null,
  max_open_managed_mrs: 1, manager_wake_autonomy: null, validation_timeout_seconds: 300
};
const remote: ChatProfile = { ...local, name: 'gah-node:worker:remote', chat_profile: 'gah-node:worker:remote',
  node_id: 'worker', remote: true, display_name: 'Remote project' };
const nodes = [
  { nodeId: 'central', displayName: 'Mac coordinator', role: 'central', chatCapable: true, eligible: true, lastSeenAt: null },
  { nodeId: 'worker', displayName: 'Windows workstation', role: 'worker', chatCapable: true, eligible: true, lastSeenAt: null },
  { nodeId: 'stale', displayName: 'Offline laptop', role: 'worker', chatCapable: false, eligible: false, reason: 'Observation is stale', lastSeenAt: null }
];
const backends = [{ id: 'claude', displayName: 'Claude', implemented: true }];

test('node readiness retries, unavailable workers stay disabled, and remote creation uses its owner', async ({ mount, page }, testInfo) => {
  let failNodes = true;
  let created: unknown;
  const modelNodes: string[] = [];
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/nodes')) return route.fulfill(failNodes
      ? { status: 503, json: { error: 'Unavailable' } } : { json: { nodes } });
    if (url.pathname.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
    if (url.pathname.endsWith('/models')) {
      modelNodes.push(url.searchParams.get('nodeId') ?? '');
      return route.fulfill({ json: { models: [], currentModelId: null } });
    }
    if (url.pathname.endsWith('/sessions')) {
      created = route.request().postDataJSON();
      return route.fulfill({ json: { id: 'created' } });
    }
    return route.fulfill({ json: {} });
  });
  const modal = await mount(<NewChatModal open currentProfile="gah" profiles={[local, remote]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await expect(modal.getByRole('button', { name: 'Retry nodes' })).toBeVisible();
  await expect(modal.getByRole('button', { name: 'Start chat' })).toBeDisabled();
  failNodes = false;
  await modal.getByRole('button', { name: 'Retry nodes' }).click();
  const picker = modal.getByRole('combobox', { name: 'Run on node' });
  await expect(picker).toHaveValue('central');
  await expect(picker.locator('option[value="stale"]')).toBeDisabled();
  await expect(picker.locator('option[value="stale"]')).toContainText('Observation is stale');
  await picker.selectOption('worker');
  await expect.poll(() => modelNodes.at(-1)).toBe('worker');
  await modal.getByRole('button', { name: 'Remote project' }).click();
  await expect(picker).toHaveValue('worker');
  await expect(modal.getByRole('tab', { name: 'From issue' })).toBeDisabled();
  await expect(modal.getByRole('tab', { name: 'From PR' })).toBeDisabled();
  await expect(modal.getByText('Each node uses its own checkout; files do not move.')).toBeVisible();
  await modal.getByRole('textbox', { name: 'Chat name' }).fill('Remote repair');
  await expect(modal.getByRole('button', { name: 'Start chat' })).toBeEnabled();
  await page.screenshot({ path: testInfo.outputPath('chat-nodes-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await picker.scrollIntoViewIfNeeded();
  expect(await modal.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('chat-nodes-mobile.png') });
  await modal.getByRole('button', { name: 'Start chat' }).click();
  await expect.poll(() => created).toEqual({ profile: remote.name, backend: 'claude', model: null, title: 'Remote repair', nodeId: 'worker' });
});

test('a late node response cannot replace the newly selected project readiness', async ({ mount, page }) => {
  const pending: (() => Promise<void>)[] = [];
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/nodes')) {
      if (url.searchParams.get('profile') === 'gah') {
        pending.push(() => route.fulfill({ json: { nodes: [nodes[0]] } }));
        return;
      }
      return route.fulfill({ json: { nodes } });
    }
    if (url.pathname.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
    return route.fulfill({ json: { models: [], currentModelId: null } });
  });
  const modal = await mount(<NewChatModal open currentProfile="gah" profiles={[local, remote]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await expect.poll(() => pending.length).toBeGreaterThan(0);
  await modal.getByRole('button', { name: 'Remote project' }).click();
  const picker = modal.getByRole('combobox', { name: 'Run on node' });
  await expect(picker).toHaveValue('worker');
  await Promise.all(pending.map(fulfill => fulfill()));
  await expect(picker).toHaveValue('worker');
  await expect(picker).toBeEnabled();
});
