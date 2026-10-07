import { expect, test } from '@playwright/test';
import type { RunningWorker } from '@git-agent-harness/contracts';
const mockUrl = `http://127.0.0.1:${process.env.GAH_MOCK_TEST_PORT ?? '3774'}`;
test.afterEach(async ({ page, request }) => {
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await request.post(`${mockUrl}/api/mock/scenario`, { data: { name: 'normal' } });
});
const worker = (run_id: string, backend: string, model: string | null, state: RunningWorker['state'] = 'running'): RunningWorker => ({ work_id: `#${run_id}`, run_id, backend, runner: backend, backend_instance: `${backend}-account`, requested_model: 'requested', model, actual_model: null, node_id: 'fixture-node', mode: 'improve', branch: `gah/${run_id}`, started_at: new Date(Date.now() - 60_000).toISOString(), last_activity_at: new Date(Date.now() - 30_000).toISOString(), attempt: 2, stale_after_seconds: 900, state });
test('all factory counters follow the roster despite conflicting claims, events and independent counters', async ({ page, request }) => {
  await request.post(`${mockUrl}/api/mock/scenario`, { data: { name: 'running-workers' } });
  await page.route('**/api/status**', async route => {
    const snapshot = await (await route.fetch()).json();
    await route.fulfill({ json: { ...snapshot, active_claims: [], inflight_implementation_count: 99 } });
  });
  await page.route('**/api/controller-activity**', route => route.fulfill({ json: Array.from({ length: 6 }, (_, index) => ({ run_id: `event-${index}`, work_id: '#other', status: 'running', profile: 'fixture', started_at: new Date().toISOString(), finished_at: null, action: 'dispatch: other', outcome: null })) }));
  await page.goto('/?page=overview&profile=fixture');
  const roster = page.getByRole('region', { name: 'Running workers' });
  await expect(roster.getByTestId('worker-row')).toHaveCount(2);
  await expect(roster.getByRole('heading')).toHaveText('Running workers (2)');
  await expect(roster).toContainText('Runner: codex · Backend: codex · Model: routed-codex');
  await expect(roster).toContainText('Runner: claude · Backend: claude · Model: routed-claude');
  await expect(roster).toContainText('stale');
  const activeWork = page.locator('.stat-tile').filter({ hasText: 'Active work' });
  await expect(activeWork.locator('.stat-tile-value')).toHaveText('2');
  await expect(page.getByRole('region', { name: 'Factory Agents Status' })).toContainText('2 busy');
  const workingMenu = page.getByRole('button', { name: /Working on.*2 jobs running/ });
  await expect(workingMenu).toBeVisible();
  await workingMenu.click();
  await expect(page.getByRole('dialog', { name: 'What the factory is doing now' }).getByRole('list').first().getByRole('listitem')).toHaveCount(2);
  await page.keyboard.press('Escape');
  await page.goto('/?page=work&profile=fixture');
  await expect(page.getByRole('region', { name: 'Running workers' }).getByTestId('worker-row')).toHaveCount(2);
  await expect(page.getByText('2 running', { exact: true })).toBeVisible();
  await expect(page.getByText('Running', { exact: true }).locator('..').locator('dd')).toHaveText('2');
  await expect(page.getByText('Capacity', { exact: true }).locator('..').locator('dd')).toHaveText(/^2\//);
  await page.setViewportSize({ width: 375, height: 812 });
  expect(await roster.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await roster.getByRole('button', { name: '#1431', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
});

test('websocket worker snapshots update the shared roster without a page reload', async ({ page }) => {
  let socket: import('@playwright/test').WebSocketRoute | undefined;
  await page.route('**/api/status**', async route => {
    const snapshot = await (await route.fetch()).json();
    await route.fulfill({ json: { ...snapshot, profile: { ...snapshot.profile, profile: 'fixture' }, running_workers: [] } });
  });
  await page.routeWebSocket('**/ws**', ws => {
    socket = ws;
    ws.send(JSON.stringify({ type: 'server.welcome', serverVersion: 'test', serverProviderCatalog: { providers: [] }, providers: {}, sessions: [], profile: 'fixture' }));
  });
  await page.goto('/?page=overview&profile=fixture');
  const roster = page.getByRole('region', { name: 'Running workers' });
  await expect(roster).toContainText('Running workers (0)');
  socket!.send(JSON.stringify({ type: 'workers.snapshot', profile: 'fixture', workers: [worker('1431', 'codex', null, 'stale')] }));
  await expect(roster).toContainText('Running workers (1)');
  await expect(roster).toContainText('Model: Unknown');
  await expect(roster).toContainText('stale');
});
