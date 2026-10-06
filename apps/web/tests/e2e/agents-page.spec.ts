import { expect, test } from '@playwright/test';

const mockUrl = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

test.beforeEach(async ({ page, request }) => {
  await request.post(`${mockUrl}/api/mock/reset`);
  await page.route('**/api/config/effective**', async (route) => {
    const response = await route.fetch();
    const config = await response.json();
    const profiles = await request.get(`${mockUrl}/api/profiles`).then((r) => r.json());
    const caps = profiles.find((p: { name: string }) => p.name === 'fixture').max_concurrent_per_model ?? {};
    await route.fulfill({ json: { ...config, improve_candidates: [{ backend: 'codex', model: Object.keys(caps)[0]?.slice(6) ?? 'gpt-5', priority: 100 }] } });
  });
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: [{ id: 'gpt-6.1-sol', name: 'GPT 6.1 Sol' }], reasoningEfforts: [],
  } }));
  await page.goto('/?page=agentpool&profile=fixture');
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
});

test('model selection shows the exact model and persists routing and capacity', async ({ page, request }) => {
  const picker = page.getByRole('combobox', { name: /^Model for Codex/ }).first();
  await expect(picker.getByRole('option', { name: 'GPT 6.1 Sol (gpt-6.1-sol)' })).toBeAttached();
  await picker.selectOption('gpt-6.1-sol');
  await page.getByLabel(/^Limit for Codex/).first().fill('2');
  await page.getByLabel('Base worker capacity').fill('4');
  const mutation = page.waitForRequest((req) => req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture'));
  await page.getByRole('button', { name: 'Save models and limits' }).click();
  expect((await mutation).postDataJSON()).toMatchObject({ agent_model: ['codex/gpt-5=gpt-6.1-sol'], max_concurrent: ['codex/gpt-6.1-sol=2'] });
  await expect(page.getByText('Agent models and limits saved.')).toBeVisible();
  const profiles = await request.get(`${mockUrl}/api/profiles`).then((r) => r.json());
  expect(profiles.find((p: { name: string }) => p.name === 'fixture').max_parallel_workers).toBe(4);
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('combobox', { name: /^Model for Codex/ }).first()).toHaveValue('gpt-6.1-sol');
});

test('manual capacity persists, can be removed, and reports save failures', async ({ page, request }) => {
  await page.getByLabel('Workers to add').fill('2');
  await page.getByLabel('Duration in hours').fill('1');
  await page.getByRole('button', { name: 'Add worker capacity', exact: true }).click();
  await expect(page.getByText('Added capacity for 2 extra workers.', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Remove extra capacity' })).toBeVisible();
  const profiles = await request.get(`${mockUrl}/api/profiles`).then((r) => r.json());
  expect(profiles.find((p: { name: string }) => p.name === 'fixture').worker_scaling).toMatchObject({ boost_workers: 2 });
  await page.getByRole('button', { name: 'Remove extra capacity' }).click();
  await expect(page.getByRole('button', { name: 'Add worker capacity', exact: true })).toBeVisible();
  await page.route('**/api/profiles/fixture', (route) => route.request().method() === 'PATCH'
    ? route.fulfill({ status: 500, json: { error: 'Capacity could not be saved' } }) : route.continue());
  await page.getByRole('button', { name: 'Add worker capacity', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Failed to save');
  await expect(page.getByText('Added capacity for', { exact: false })).toHaveCount(0);
});

test('capacity controls fit a phone viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByLabel('Workers to add')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/agents-capacity-mobile.png', fullPage: true });
});

test('expired capacity can be replaced and the work loop can be started', async ({ page, request }) => {
  await request.patch(`${mockUrl}/api/profiles/fixture`, { data: { boost_workers: 2, boost_hours: -1 } });
  await request.post(`${mockUrl}/api/loop/stop`);
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Add worker capacity', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Start work loop' }).click();
  await expect(page.getByText('Work loop running', { exact: true })).toBeVisible();
});

test('an unlisted model is editable and catalog failure keeps the current selection', async ({ page }) => {
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ status: 503, json: { error: 'Unavailable' } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByText('Couldn’t load models.', { exact: false })).toBeVisible();
  const picker = page.getByRole('combobox', { name: /^Model for Codex/ }).first();
  await expect(picker).toHaveValue('gpt-5');
  await picker.selectOption('__custom__');
  await page.getByRole('textbox', { name: /^Custom model for codex/ }).fill('custom-model');
  await page.getByRole('button', { name: 'Save models and limits' }).click();
  await expect(page.getByText('Agent models and limits saved.')).toBeVisible();
});
