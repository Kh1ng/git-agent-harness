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

async function openChat(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

async function selectSeededSession(page: Page): Promise<void> {
  const sessions = page.getByLabel('Chat session');
  await sessions.selectOption('mock-session-1');
  await expect(sessions).toHaveValue('mock-session-1');
  await expect(page.getByLabel('Session provider')).toHaveValue('codex');
}

// The mock is one shared stateful process. Keep scenario mutations ordered;
// this file intentionally contains no page.route/routeWebSocket shims.
test.describe.configure({ mode: 'serial' });

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

  await page.getByPlaceholder(/Message the manager/).fill('resume this turn');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Before disconnect…', { exact: true })).toBeVisible();
  await expect.poll(async () => {
    const response = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
    return (await response.json() as { connections: number }).connections;
  }, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
  await expect(page.getByText('Before disconnect… resumed after reconnect.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('disconnect restores a pending permission card and resumes actionably', async ({ page, request }) => {
  test.setTimeout(25_000);
  await selectScenario(request, 'reconnect-permission');
  await openChat(page);

  await page.getByPlaceholder(/Message the manager/).fill('disconnect while permission is pending');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run focused Playwright tests');

  // The mock closes the real socket. Wait for the app's own reconnect loop,
  // then prove history restored both busy state and the actionable card.
  await expect.poll(async () => {
    const response = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
    return (await response.json() as { connections: number }).connections;
  }, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
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
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  await expect(page.getByLabel('Chat session').locator('optgroup[label^="Archived"]')).toContainText('Mock session');

  await selectScenario(request, 'archive-failure');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  await expect(page.getByText('Failed to archive session: Mock archive failed')).toBeVisible();
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
