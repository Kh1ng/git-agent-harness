import { expect, test } from '@playwright/test';

const mockUrl = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

test.afterEach(async ({ page }) => {
  await page.unrouteAll({ behavior: 'wait' });
});

test.beforeEach(async ({ page, request }) => {
  await request.post(`${mockUrl}/api/mock/reset`);
  let configuredModel = 'gpt-5';
  page.on('request', (req) => {
    if (req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture')) {
      const change = req.postDataJSON()?.agent_model?.[0] as string | undefined;
      if (change) configuredModel = change.slice(change.indexOf('=') + 1);
    }
  });
  await page.route('**/api/config/effective**', async (route) => {
    const response = await route.fetch();
    const config = await response.json();
    await route.fulfill({ json: { ...config, improve_candidates: [{ backend: 'codex', model: configuredModel, priority: 100 }] } });
  });
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: [{ id: 'gpt-6.1-sol', name: 'GPT 6.1 Sol' }], reasoningEfforts: [],
  } }));
  await page.goto('/?page=agentpool&profile=fixture');
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await page.getByText('Automatic scaling & capacity settings', { exact: true }).click();
});

test('model selection shows the exact model and persists routing and capacity', async ({ page, request }) => {
  const picker = page.getByRole('combobox', { name: /^Model for Codex/ }).first();
  await expect(picker.getByRole('option', { name: 'GPT 6.1 Sol' })).toBeAttached();
  await picker.selectOption('gpt-6.1-sol');
  await page.getByLabel(/^Limit for Codex/).first().fill('2');
  await page.getByLabel('Base worker capacity').fill('4');
  const mutation = page.waitForRequest((req) => req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture'));
  await page.getByRole('button', { name: 'Save agent settings' }).click();
  expect((await mutation).postDataJSON()).toMatchObject({ agent_model: ['codex/gpt-5=gpt-6.1-sol'], max_concurrent: ['codex/gpt-6.1-sol=2'] });
  await expect(page.getByText('Agent settings saved.')).toBeVisible();
  const profiles = await request.get(`${mockUrl}/api/profiles`).then((r) => r.json());
  expect(profiles.find((p: { name: string }) => p.name === 'fixture').max_parallel_workers).toBe(4);
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('combobox', { name: /^Model for Codex/ }).first()).toHaveValue('gpt-6.1-sol');
  await page.screenshot({ path: 'test-results/agents-capacity-desktop.png' });
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
  await page.getByLabel('Workers to add').scrollIntoViewIfNeeded();
  await page.screenshot({ path: 'test-results/agents-extra-capacity-mobile.png' });
});

test('expired capacity can be replaced and the work loop can be started', async ({ page, request }) => {
  await request.patch(`${mockUrl}/api/profiles/fixture`, { data: { boost_workers: 2, boost_hours: -1 } });
  await request.post(`${mockUrl}/api/loop/stop`);
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await page.getByText('Automatic scaling & capacity settings', { exact: true }).click();
  await expect(page.getByRole('button', { name: 'Add worker capacity', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Start work loop' }).click();
  await expect(page.getByText('Work loop running', { exact: true })).toBeVisible();
});

test('an unlisted model is editable and catalog failure keeps the current selection', async ({ page }) => {
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ status: 503, json: { error: 'Unavailable' } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByText('Couldn’t load models.', { exact: false }).first()).toBeVisible();
  const picker = page.getByRole('combobox', { name: /^Model for Codex/ }).first();
  await expect(picker).toHaveValue('gpt-5');
  await picker.selectOption('__custom__');
  await page.getByRole('textbox', { name: /^Custom model for codex/ }).fill('custom-model');
  await page.getByRole('button', { name: 'Save agent settings' }).click();
  await expect(page.getByText('Agent settings saved.')).toBeVisible();
});

test('Claude models show versions and context variants while saving exact model values', async ({ page }) => {
  await page.route('**/api/config/effective**', async (route) => {
    const config = await (await route.fetch()).json();
    await route.fulfill({ json: { ...config, improve_candidates: [{ backend: 'claude', model: 'opus', priority: 100 }] } });
  });
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: [
      { id: 'opus[1m]', name: 'Opus (1M context)', description: 'Opus 5 with 1M context · Best for complex tasks' },
      { id: 'claude-opus-4-6', name: 'Opus', description: 'A pinned version' },
      { id: 'sonnet', name: 'Sonnet', description: 'Sonnet 5 · Efficient for routine tasks' },
      { id: 'haiku', name: 'Haiku', description: 'Haiku 4.5 · Fastest for quick answers' },
      { id: 'claude-fable-5[1m]', name: 'Fable', description: 'Fable 5' },
    ], reasoningEfforts: [],
  } }));
  await page.route('**/api/report/roles**', (route) => route.fulfill({ json: {
    model_aliases: [
      { backend: 'claude', alias: 'opus[1m]', model: '<synthetic>' },
      { backend: 'claude', alias: 'opus', model: 'claude-opus-5-5' },
      { backend: 'claude', alias: 'sonnet', model: 'claude-sonnet-4-6' },
      { backend: 'claude', alias: 'claude-fable-5[1m]', model: 'claude-fable-5-1' },
    ],
  } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  const picker = page.getByRole('combobox', { name: /^Model for Claude/ });
  await expect(picker.getByRole('option', { name: 'Opus 5.5 (1M context)', exact: true })).toBeAttached();
  await expect(picker.getByRole('option', { name: 'Opus 4.6', exact: true })).toBeAttached();
  await expect(picker.getByRole('option', { name: 'Sonnet 5', exact: true })).toBeAttached();
  await expect(picker.getByRole('option', { name: 'Haiku 4.5', exact: true })).toBeAttached();
  await expect(picker.getByRole('option', { name: 'Fable 5.1 (1M context)', exact: true })).toBeAttached();
  await expect(picker.getByRole('option', { selected: true })).toHaveText('Opus 5.5 (provider default)');
  await picker.selectOption('claude-opus-4-6');
  const mutation = page.waitForRequest((req) => req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture'));
  await page.getByRole('button', { name: 'Save agent settings' }).click();
  expect((await mutation).postDataJSON()).toMatchObject({ agent_model: ['claude/opus=claude-opus-4-6'] });
  await expect(page.getByText('Agent settings saved.')).toBeVisible();
});

test('Antigravity separates model family from reasoning and saves the exact provider variant', async ({ page }) => {
  await page.route('**/api/config/effective**', async (route) => {
    const config = await (await route.fetch()).json();
    await route.fulfill({ json: { ...config, improve_candidates: [{ backend: 'agy', model: 'Gemini 3.8 Flash (High)', priority: 100 }] } });
  });
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: ['High', 'Medium', 'Low'].map((effort) => ({ id: `gemini-3.8-flash-${effort.toLowerCase()}`, name: `Gemini 3.8 Flash (${effort})` })),
    reasoningEfforts: ['low', 'medium', 'high'].map((id) => ({ id, name: id })),
  } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  const picker = page.getByRole('combobox', { name: /^Model for Antigravity/ });
  await expect(picker.getByRole('option')).toHaveCount(2); // one model + custom entry
  await expect(picker.getByRole('option', { selected: true })).toHaveText('Gemini 3.8 Flash');
  const reasoning = page.getByRole('combobox', { name: /^Reasoning for Antigravity/ });
  await expect(reasoning).toHaveValue('high');
  await expect(reasoning.getByRole('option')).toHaveText(['Low', 'Medium', 'High']);
  await reasoning.selectOption('low');
  await expect(picker).toHaveAttribute('title', /Model ID: gemini-3.8-flash-low/);
  await page.getByRole('region', { name: 'Models, reasoning & worker limits' }).getByText('Model details', { exact: true }).first().click();
  await expect(page.getByText('Provider name: Gemini 3.8 Flash (Low)', { exact: false })).toBeVisible();
  const mutation = page.waitForRequest((req) => req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture'));
  await page.getByRole('button', { name: 'Save agent settings' }).click();
  expect((await mutation).postDataJSON()).toMatchObject({ agent_model: ['agy/Gemini 3.8 Flash (High)=Gemini 3.8 Flash (Low)'] });
});

test('Codex reasoning persists independently of model and uses consistent GPT capitalization', async ({ page }) => {
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: [{ id: 'gpt-6.1-sol', name: 'Gpt 6.1 Sol' }], reasoningEfforts: [{ id: 'high', name: 'High' }],
  } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('combobox', { name: /^Model for Codex/ }).getByRole('option', { name: 'GPT 6.1 Sol' })).toBeAttached();
  await page.getByRole('combobox', { name: /^Reasoning for Codex/ }).selectOption('high');
  const mutation = page.waitForRequest((req) => req.method() === 'PATCH' && req.url().endsWith('/api/profiles/fixture'));
  await page.getByRole('button', { name: 'Save agent settings' }).click();
  expect((await mutation).postDataJSON()).toMatchObject({ agent_effort: ['codex=high'], agent_model: [] });
  await expect(page.getByText('Agent settings saved.')).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('combobox', { name: /^Reasoning for Codex/ })).toHaveValue('high');
});

test('manual start defaults to the next job, allows a specific job, and reports launch errors', async ({ page }) => {
  await page.route('**/api/status**', async (route) => {
    const status = await (await route.fetch()).json();
    await route.fulfill({ json: { ...status, available_tickets: ['first', 'second'].map((name) => ({
      ticket_path: `${name}.md`, title: `${name} queued job`, has_active_claim: false, has_active_mr: false, human_required: false,
      execution_policy: { dispatchable_now: true },
    })) } });
  });
  await page.route('**/api/manager-chat/models**', (route) => route.fulfill({ json: {
    models: [{ id: 'gpt-6.1-sol', name: 'GPT 6.1 Sol' }], reasoningEfforts: [{ id: 'high', name: 'High' }],
  } }));
  await page.reload();
  await page.getByRole('button', { name: 'Models & capacity', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Worker job' })).toHaveValue('');
  await expect(page.getByRole('paragraph').filter({ hasText: /^first queued job$/ })).toBeVisible();
  await page.getByRole('combobox', { name: 'Worker model', exact: true }).selectOption('gpt-6.1-sol');
  await page.getByRole('combobox', { name: 'Worker reasoning level' }).selectOption('high');
  await page.getByRole('combobox', { name: 'Worker job' }).selectOption('second.md');
  await page.route('**/api/dispatch', (route) => route.fulfill({ status: 202, json: { session: {
    id: 'manual-worker-1', status: 'error', error: 'Subscriber is signed out', target: 'second.md', providerKind: 'github', instanceId: 'github-0',
  } } }));
  const start = page.waitForRequest((req) => req.method() === 'POST' && req.url().endsWith('/api/dispatch'));
  await page.getByRole('button', { name: 'Start worker', exact: true }).click();
  expect((await start).postDataJSON()).toMatchObject({ backend: 'codex', model: 'gpt-6.1-sol', reasoningEffort: 'high', manualWorker: true, target: 'second.md', mode: 'improve' });
  await expect(page.getByRole('alert')).toContainText('Subscriber is signed out');
  await page.route('**/api/dispatch', (route) => route.fulfill({ status: 502, json: { error: 'Provider unavailable' } }));
  await page.getByRole('button', { name: 'Start worker', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Worker could not start');
});
