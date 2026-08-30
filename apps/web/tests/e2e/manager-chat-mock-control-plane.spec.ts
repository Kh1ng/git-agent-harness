import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

type Scenario =
  | 'normal'
  | 'slow-cancel-steer'
  | 'reconnect-stream'
  | 'reconnect-permission'
  | 'archive-success'
  | 'archive-failure'
  | 'preview-unavailable'
  | 'preview-available'
  | 'models-success'
  | 'models-empty'
  | 'models-delayed'
  | 'models-failure'
  | 'models-agy';

async function selectScenario(request: APIRequestContext, name: Scenario): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

async function connectionCount(request: APIRequestContext): Promise<number> {
  const response = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
  expect(response.ok(), await response.text()).toBe(true);
  return (await response.json() as { connections: number }).connections;
}

async function openChat(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

async function selectSeededSession(page: Page): Promise<void> {
  await page.getByLabel('Chat session').click();
  await page.getByRole('option', { name: 'Mock session', exact: true }).click();
  await expect(page.getByLabel('Session provider')).toHaveValue('codex');
}

/** The picker's two top-level sections, for asserting where a session lives. */
function pickerSection(page: Page, name: 'Active' | 'Archive') {
  return page.getByRole('listbox', { name: 'All sessions' }).getByRole('group', { name });
}

// The mock is one shared stateful process. Keep scenario mutations ordered.
test.describe.configure({ mode: 'serial' });

test('active picker discovers sessions created through REST', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const response = await request.post(`${MOCK_BASE_URL}/api/manager-chat/sessions`, {
    data: { profile: 'fixture', backend: 'codex', title: 'Externally created session' }
  });
  expect(response.ok(), await response.text()).toBe(true);

  await page.getByLabel('Chat session').click();
  await expect(page.getByRole('option', { name: 'Externally created session' })).toBeVisible({ timeout: 10_000 });
});

test('shared mock streams multiple chunks, tool activity, and completion', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('exercise normal streaming');
  await page.getByRole('button', { name: 'Send' }).click();

  await expect(page.getByText('Mock turn', { exact: false })).toBeVisible();
  await expect(page.getByText('Read fixture contracts', { exact: true })).toBeVisible();
  await expect(page.getByText('Mock turn complete after multiple chunks.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('streaming resumes after the mock drops and restores the real socket', async ({ page, request }) => {
  test.setTimeout(25_000);
  await selectScenario(request, 'reconnect-stream');
  await openChat(page);
  const connectionsBeforeTurn = await connectionCount(request);

  await page.getByPlaceholder(/Message the manager/).fill('resume this turn');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Before disconnect…', { exact: true })).toBeVisible();
  await expect
    .poll(() => connectionCount(request), { timeout: 15_000 })
    .toBeGreaterThan(connectionsBeforeTurn);
  await expect(page.getByText('Before disconnect… resumed after reconnect.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('disconnect restores a pending permission card and resumes actionably', async ({ page, request }) => {
  test.setTimeout(25_000);
  await selectScenario(request, 'reconnect-permission');
  await openChat(page);
  const connectionsBeforeTurn = await connectionCount(request);

  await page.getByPlaceholder(/Message the manager/).fill('disconnect while permission is pending');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run focused Playwright tests');

  // The mock closes the real socket. Wait for the app's own reconnect loop,
  // then prove history restored both busy state and the actionable card.
  await expect
    .poll(() => connectionCount(request), { timeout: 15_000 })
    .toBeGreaterThan(connectionsBeforeTurn);
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run focused Playwright tests');

  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByText('Permission allow-once received after reconnect.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('slow turn accepts/rejects steering and cancellation restores idle', async ({ page, request }) => {
  await selectScenario(request, 'slow-cancel-steer');
  await openChat(page);

  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('start slowly');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Working slowly…', { exact: true })).toBeVisible();

  await composer.fill('change direction');
  await composer.press('Enter');
  await expect(page.getByText('change direction', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();

  await composer.fill('reject this direction');
  await composer.press('Enter');
  await expect(page.getByText('Steering failed: Mock backend rejected steering')).toBeVisible();

  await page.getByRole('button', { name: 'Stop' }).click();
  await expect(page.getByText('[cancelled]', { exact: true })).toBeVisible();
  await composer.fill('after cancel');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
});

test('archive and preview states mutate through the same REST control plane', async ({ page, request }) => {
  await selectScenario(request, 'preview-unavailable');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(page.getByText('No preview yet.', { exact: false })).toBeVisible();

  await selectScenario(request, 'preview-available');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Preview :4173', exact: true }).click();
  await expect(page.getByTitle('Session preview')).toBeVisible();
  await expect(page.frameLocator('iframe[title="Session preview"]').getByRole('heading', { name: 'Mock preview available' })).toBeVisible();

  await selectScenario(request, 'archive-success');
  await openChat(page);
  await selectSeededSession(page);

  let delayNextSessionList = true;
  await page.route('**/api/manager-chat/sessions?**', async (route) => {
    if (!delayNextSessionList) return route.continue();
    delayNextSessionList = false;
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    await route.continue();
  });
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  const trigger = page.getByLabel('Chat session');
  // The selection resets optimistically, before any refresh lands.
  await expect(trigger).toContainText('Default conversation', { timeout: 750 });

  const archived = await request.get(`${MOCK_BASE_URL}/api/manager-chat/sessions?profile=fixture`);
  expect(archived.ok(), await archived.text()).toBe(true);
  const archivedSessions = (await archived.json() as { sessions: { id: string; archivedAt: number | null }[] }).sessions;
  expect(archivedSessions.find((session) => session.id === 'mock-session-1')?.archivedAt).not.toBeNull();

  // The picker now lists the archived session under Archive — and only there.
  await trigger.click();
  await expect(pickerSection(page, 'Archive').getByRole('option', { name: 'Mock session', exact: true })).toBeVisible({ timeout: 10_000 });
  await expect(pickerSection(page, 'Active').getByRole('option', { name: 'Mock session', exact: true })).toHaveCount(0);

  // The 5s auto-refresh keeps it archived: it never resurfaces as active.
  await page.waitForTimeout(5_500);
  await expect(pickerSection(page, 'Archive').getByRole('option', { name: 'Mock session', exact: true })).toBeVisible();
  await expect(pickerSection(page, 'Active').getByRole('option', { name: 'Mock session', exact: true })).toHaveCount(0);

  await selectScenario(request, 'archive-success');
  const connectionsBeforeReconnect = await connectionCount(request);
  const rearchived = await request.post(`${MOCK_BASE_URL}/api/manager-chat/sessions/archive`, {
    data: { profile: 'fixture', sessionId: 'mock-session-1' }
  });
  expect(rearchived.ok(), await rearchived.text()).toBe(true);
  await expect.poll(() => connectionCount(request), { timeout: 15_000 }).toBeGreaterThan(connectionsBeforeReconnect);
  await expect(trigger).toContainText('Default conversation');
  await expect(pickerSection(page, 'Active').getByRole('option', { name: 'Mock session', exact: true })).toHaveCount(0);

  await selectScenario(request, 'archive-failure');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  await expect(page.getByText('Failed to archive session: Mock archive failed')).toBeVisible();
  await expect(page.getByLabel('Chat session')).toContainText('Mock session');
});

test('storage dry run selects idle sessions and bulk archives them safely', async ({ page, request }) => {
  await selectScenario(request, 'archive-success');
  await openChat(page);

  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  const storage = page.getByRole('region', { name: 'Chat storage' });
  await expect(storage).toContainText('12.0 MiB in worktrees');
  await expect(storage).toContainText('12.0 MiB projected reclaim');
  await expect(storage).toContainText('archive · idle');

  await storage.getByRole('button', { name: 'Select idle (1)' }).click();
  await expect(storage.getByLabel('Select Mock session')).toBeChecked();
  await storage.getByRole('button', { name: 'Archive selected (1)' }).click();
  await page.getByLabel('Chat session').click();
  await expect(pickerSection(page, 'Archive').getByRole('option', { name: 'Mock session', exact: true })).toBeVisible({ timeout: 10_000 });
  await expect(storage).toContainText('No live chat sessions.');
});

test('backend/model controls cover success, delayed, empty, failed, and AGY shapes', async ({ page, request }) => {
  await selectScenario(request, 'models-success');
  await openChat(page);
  await expect(page.getByLabel('Harness / backend')).toHaveValue('codex');
  await expect(page.getByLabel('Model')).toHaveValue('gpt-5.3-codex');
  await expect(page.getByLabel('Reasoning effort')).toHaveValue('medium');

  await selectScenario(request, 'models-delayed');
  await openChat(page);
  await expect(page.getByLabel('Model')).toHaveCount(0);
  await expect(page.getByLabel('Model')).toHaveValue('gpt-5.3-codex', { timeout: 5_000 });

  await selectScenario(request, 'models-empty');
  await openChat(page);
  await expect(page.getByText('Default model · Codex', { exact: true })).toBeVisible();

  await selectScenario(request, 'models-failure');
  await openChat(page);
  await expect(page.getByText('Default model · Codex', { exact: true })).toBeVisible();

  await selectScenario(request, 'models-agy');
  await openChat(page);
  await expect(page.getByLabel('Harness / backend')).toHaveValue('agy');
  await expect(page.getByText('Default model · AGY', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Reasoning effort')).toHaveCount(0);
});

test('session controls pin and switch the session reasoning effort', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await selectSeededSession(page);

  // The mock codex advertises low/medium/xhigh, so the session-level picker
  // renders next to the session model picker.
  const effort = page.getByLabel('Session reasoning effort');
  await expect(effort).toBeVisible();
  await expect(effort).toHaveValue('');

  await effort.selectOption('xhigh');
  await expect(effort).toHaveValue('xhigh');
  const state = await (await request.get(`${MOCK_BASE_URL}/api/mock/state`)).json() as {
    sessions: { id: string; reasoningEffort: string | null }[];
  };
  expect(state.sessions.find((s) => s.id === 'mock-session-1')?.reasoningEffort).toBe('xhigh');
});
