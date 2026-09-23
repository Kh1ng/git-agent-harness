import { expect, test, type WebSocketRoute } from '@playwright/test';

test('chat sends on the chosen node, locks it during a turn, and refreshes readiness from fleet events', async ({ page }) => {
  const local = { name: 'alpha', display_name: 'Alpha', repo: 'org/alpha', provider: 'github' };
  const remote = { ...local, name: 'remote', display_name: 'Remote project', node_id: 'worker', chat_profile: 'gah-node:worker:remote' };
  let stale = false;
  let nodeReads = 0;
  const sent: { requestId: string; profile: string; nodeId: string; message: string }[] = [];
  const created: Record<string, unknown>[] = [];
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/profiles') return route.fulfill({ json: [local] });
    if (path === '/api/projects') return route.fulfill({ json: [remote] });
    if (path === '/api/manager-chat/nodes') {
      nodeReads++;
      return route.fulfill({ json: { nodes: [
        { nodeId: 'central', displayName: 'Coordinator', role: 'central', eligible: true, chatCapable: true, lastSeenAt: null },
        { nodeId: 'worker', displayName: 'Windows workstation', role: 'worker', eligible: !stale, chatCapable: !stale, reason: stale ? 'Observation is stale' : null, lastSeenAt: null }
      ] } });
    }
    if (path === '/api/manager-chat/storage') return route.fulfill({ json: {
      profiles: [{ profile: 'alpha', worktreeBytes: null, projectedReclaimBytes: null, idleDays: 7, sessions: [] }],
      candidates: [], warnings: []
    } });
    if (path === '/api/manager-chat/sessions') {
      // A blank chat's first message creates its session before it sends.
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON() as Record<string, unknown>;
        created.push(body);
        return route.fulfill({ json: {
          id: 'created-session', profile: body.profile, nodeId: body.nodeId, worktreePath: null,
          branch: 'gah/chat/created', backend: 'codex', model: null, reasoningEffort: null,
          title: 'Run on the imported owner', createdAt: 0, lastActiveAt: 0, archivedAt: null,
          outcome: 'live', settledAt: null, settledReason: null
        } });
      }
      return route.fulfill({ json: { sessions: [] } });
    }
    if (path === '/api/manager-chat/settings') return route.fulfill({ json: {
      defaultBackend: 'codex', profileOverrides: {}, availableBackends: [{ id: 'codex', displayName: 'Codex', implemented: true }]
    } });
    if (path === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (path === '/api/manager-chat/models') return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.continue();
  });
  let socket: WebSocketRoute;
  await page.routeWebSocket('**/ws**', ws => {
    socket = ws;
    ws.send(JSON.stringify({ type: 'server.welcome', serverVersion: 'test', serverProviderCatalog: { providers: [] }, sessions: [], providers: {}, profile: 'alpha' }));
    ws.onMessage(raw => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.send') sent.push(message);
      if (message.type === 'manager.chat.historyRequest') ws.send(JSON.stringify({
        type: 'manager.chat.history', requestId: message.requestId, profile: message.profile,
        turns: [], cursor: 0, streaming: null, permission: null
      }));
    });
  });
  // Chat opens a blank conversation; these turns belong to the project's
  // default conversation, which the Projects page links to directly.
  await page.goto('/?page=chat&profile=alpha&chat=default');
  const picker = page.getByRole('combobox', { name: 'Run on node' });
  await expect(picker).toHaveValue('central');
  await picker.selectOption('worker');
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Chat storage' })).toContainText('Unknown in worktrees · Unknown projected reclaim');
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  await page.getByPlaceholder(/Message the manager/).fill('Check the worker checkout');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0].nodeId).toBe('worker');
  await expect(picker).toBeDisabled();
  socket!.send(JSON.stringify({ type: 'manager.chat.reply', requestId: sent[0].requestId, profile: 'alpha', reply: 'Worker checked', backend: 'codex', model: null, nodeId: 'worker', nodeName: 'Worker execution host' }));
  await expect(page.getByText('codex · Worker execution host', { exact: true })).toBeVisible();
  await expect(picker).toBeEnabled();
  const readsBefore = nodeReads;
  stale = true;
  socket!.send(JSON.stringify({ type: 'fleet.changed' }));
  await expect.poll(() => nodeReads).toBeGreaterThan(readsBefore);
  await expect(page.getByRole('status').filter({ hasText: 'Observation is stale' })).toBeVisible();
  await page.getByPlaceholder(/Message the manager/).fill('Must not silently move to central');
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
  stale = false;
  socket!.send(JSON.stringify({ type: 'fleet.changed' }));
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
  // Switching project happens in a blank chat's launcher; the imported
  // project keeps running on its owning node.
  await page.getByRole('button', { name: 'New chat', exact: true }).click();
  await page.getByRole('list').filter({ hasText: 'org/alpha' }).getByRole('button', { name: /Remote project/ }).click();
  await expect(picker).toHaveValue('worker');
  await page.getByPlaceholder(/Message the manager/).fill('Run on the imported owner');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect.poll(() => sent.length).toBe(2);
  expect(sent[1].profile).toBe(remote.chat_profile);
  expect(sent[1].nodeId).toBe('worker');
  // The blank chat names itself after its first message and opens on the
  // project's owning node, not the coordinator.
  expect(created).toEqual([{
    profile: remote.chat_profile, nodeId: 'worker', backend: 'codex', model: null,
    title: 'Run on the imported owner'
  }]);
});
