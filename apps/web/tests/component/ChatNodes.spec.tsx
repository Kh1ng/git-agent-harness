import { expect, test, type Locator } from '@playwright/experimental-ct-react';
import React from 'react';
import { NewChatPanel, type ChatProfile } from '../../src/components/NewChatPanel.js';

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

/** Project, node and provider sit behind "Extra settings"; a node problem opens it on its own. */
async function openExtras(panel: Locator): Promise<void> {
  const details = panel.locator('details');
  if (!await details.evaluate((element) => (element as HTMLDetailsElement).open)) await details.locator('summary').click();
}

for (const source of ['issue', 'pr'] as const) {
  test(`${source} creation replaces an offline owner and preserves the chosen node and account`, async ({ mount, page }) => {
    let started: unknown;
    await page.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith('/nodes')) return route.fulfill({ json: { nodes } });
      if (path.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
      if (path.endsWith('/backend-instances')) return route.fulfill({ json: { backend_instances: [{ backend_instance: 'claude-2', logical_backend: 'claude', enabled: true, executable_resolved: true, auth_ready: true, account_label: 'Second login' }] } });
      if (path.endsWith(`/${source}s`)) return route.fulfill({ json: { [`${source}s`]: [{ number: 7, title: 'Source task', labels: [], url: null, author: null, headRefName: null, isDraft: false, reviewState: null, updatedAt: null }] } });
      if (path.endsWith('/start')) {
        started = route.request().postDataJSON();
        return route.fulfill({ json: { session: { id: 'created' } } });
      }
      return route.fulfill({ json: { models: [], currentModelId: null } });
    });
    const project: ChatProfile = { ...remote, name: 'gah-node:stale:remote', node_id: 'stale' };
    const modal = await mount(<NewChatPanel currentProfile={project.name} profiles={[project]} backends={backends}
      onClose={() => {}} onCreated={() => {}} />);
    await modal.getByRole('tab', { name: source === 'issue' ? 'From issue' : 'From PR' }).click();
    await openExtras(modal);
    const picker = modal.getByRole('combobox', { name: 'Run on node' });
    await expect(picker).toHaveValue('central');
    await picker.selectOption('worker');
    await modal.getByRole('combobox', { name: 'Account' }).selectOption('claude-2');
    await modal.getByRole('button', { name: '#7 Source task' }).click();
    await expect.poll(() => started).toMatchObject({ profile: project.name, [`${source}Number`]: 7, nodeId: 'worker', backendInstance: 'claude-2' });
  });
}

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
  const modal = await mount(<NewChatPanel currentProfile="gah" profiles={[local, remote]} backends={backends}
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
  await expect(modal.getByRole('tab', { name: 'From issue' })).toBeEnabled();
  await expect(modal.getByRole('tab', { name: 'From PR' })).toBeEnabled();
  await expect(modal.getByText('Each node uses its own checkout; files do not move.')).toBeVisible();
  await modal.getByRole('textbox', { name: 'Chat name' }).fill('Remote repair');
  await expect(modal.getByRole('button', { name: 'Start chat' })).toBeEnabled();
  await page.screenshot({ path: testInfo.outputPath('chat-nodes-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await picker.scrollIntoViewIfNeeded();
  expect((await picker.boundingBox())!.height).toBeGreaterThanOrEqual(44);
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
  const modal = await mount(<NewChatPanel currentProfile="gah" profiles={[local, remote]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await expect.poll(() => pending.length).toBeGreaterThan(0);
  await openExtras(modal);
  await modal.getByRole('button', { name: 'Remote project' }).click();
  const picker = modal.getByRole('combobox', { name: 'Run on node' });
  await expect(picker).toHaveValue('worker');
  await Promise.all(pending.map(fulfill => fulfill()));
  await expect(picker).toHaveValue('worker');
  await expect(picker).toBeEnabled();
});

test('a project whose owner is unavailable defaults to an eligible node (#1275)', async ({ mount, page }) => {
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/nodes')) return route.fulfill({ json: { nodes } });
    if (url.pathname.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
    if (url.pathname.endsWith('/models')) return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.fulfill({ json: {} });
  });
  const ownedByStale: ChatProfile = { ...local, node_id: 'stale' };
  const modal = await mount(<NewChatPanel currentProfile="gah" profiles={[ownedByStale]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await openExtras(modal);
  await expect(modal.getByRole('combobox', { name: 'Run on node' })).toHaveValue('central');
  await modal.getByRole('textbox', { name: 'Chat name' }).fill('Blank chat');
  await expect(modal.getByRole('button', { name: 'Start chat' })).toBeEnabled();
});

test('a worker-only project starts a chat from an issue (#1276)', async ({ mount, page }) => {
  let started: unknown;
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/nodes')) return route.fulfill({ json: { nodes } });
    if (url.pathname.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
    if (url.pathname.endsWith('/models')) return route.fulfill({ json: { models: [], currentModelId: null } });
    if (url.pathname.endsWith('/issues')) return route.fulfill({ json: { issues: [{ number: 7, title: 'Worker bug', url: 'https://example/7', labels: [], updatedAt: null, inProgress: false }] } });
    if (url.pathname.endsWith('/issues/start')) {
      started = route.request().postDataJSON();
      return route.fulfill({ status: 201, json: { session: { id: 's1' }, existing: false } });
    }
    return route.fulfill({ json: {} });
  });
  const modal = await mount(<NewChatPanel currentProfile={remote.name} profiles={[remote]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await modal.getByRole('tab', { name: 'From issue' }).click();
  await openExtras(modal);
  await expect(modal.getByRole('combobox', { name: 'Run on node' })).toHaveValue('worker');
  // Picking an issue starts the chat immediately.
  await modal.getByText('Worker bug').click();
  await expect.poll(() => started).toMatchObject({ profile: remote.name, issueNumber: 7, backend: 'claude' });
});

test('a worker-only project starts a chat from a pull request (#1276)', async ({ mount, page }) => {
  let started: unknown;
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/nodes')) return route.fulfill({ json: { nodes } });
    if (url.pathname.endsWith('/settings')) return route.fulfill({ json: { profileOverrides: {}, defaultBackend: 'claude' } });
    if (url.pathname.endsWith('/models')) return route.fulfill({ json: { models: [], currentModelId: null } });
    if (url.pathname.endsWith('/prs')) return route.fulfill({ json: { prs: [{ number: 9, title: 'Worker fix', url: 'https://example/9', author: 'octo', headRefName: 'fix/worker', isDraft: false, reviewState: null, updatedAt: null }] } });
    if (url.pathname.endsWith('/prs/start')) {
      started = route.request().postDataJSON();
      return route.fulfill({ status: 201, json: { session: { id: 's2' }, existing: false } });
    }
    return route.fulfill({ json: {} });
  });
  const modal = await mount(<NewChatPanel currentProfile={remote.name} profiles={[remote]} backends={backends}
    onClose={() => {}} onCreated={() => {}} />);
  await modal.getByRole('tab', { name: 'From PR' }).click();
  await openExtras(modal);
  await expect(modal.getByRole('combobox', { name: 'Run on node' })).toHaveValue('worker');
  await modal.getByText('Worker fix').click();
  await expect.poll(() => started).toMatchObject({ profile: remote.name, prNumber: 9, backend: 'claude' });
});
