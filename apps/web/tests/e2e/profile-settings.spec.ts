import { expect, test } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

test('the Profile sidebar exposes validation timeout and persists profile updates in the shared mock', async ({ page, request }) => {
  test.setTimeout(120_000);
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);

  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.evaluate(() => localStorage.clear());
  await page.reload({ waitUntil: 'domcontentloaded' });
  const profileButton = page.getByRole('button', { name: 'Profile', exact: true });
  await expect(profileButton).toBeVisible({ timeout: 60_000 });
  await profileButton.click();

  const validationTimeoutInput = page
    .getByText('Validation command timeout (seconds)')
    .locator('..')
    .locator('input');
  await page.getByRole('button', { name: /^Project:/ }).click();
  await page.getByRole('menuitemradio', { name: /^Fixture/ }).click();
  await expect(page.getByText(/Per-profile factory behavior for/)).toBeVisible();

  await expect(validationTimeoutInput).toBeVisible();
  await expect(validationTimeoutInput).toHaveValue('300');
  await expect(page.getByText(/validation command timeout/i)).toBeVisible();
  await expect(page.getByText(/backend idle timeouts/i)).toBeVisible();
  await validationTimeoutInput.fill('0');
  await expect(page.getByRole('alert')).toContainText(/whole number of seconds greater than zero/i);
  await expect(page.getByRole('button', { name: 'Save dispatch settings' })).toBeDisabled();

  await validationTimeoutInput.fill('900');
  await page.getByRole('button', { name: 'Save dispatch settings' }).click();

  await expect.poll(async () => {
    const state = await request.get(`${MOCK_BASE_URL}/api/mock/state`).then((response) => response.json()) as {
      profiles: { name: string; validation_timeout_seconds: number }[];
    };
    return state.profiles.find((profile) => profile.name === 'fixture')?.validation_timeout_seconds;
  }).toBe(900);

  await validationTimeoutInput.fill('');
  await page.getByRole('button', { name: 'Save dispatch settings' }).click();

  await expect.poll(async () => {
    const state = await request.get(`${MOCK_BASE_URL}/api/mock/state`).then((response) => response.json()) as {
      profiles: { name: string; validation_timeout_seconds: number }[];
    };
    return state.profiles.find((profile) => profile.name === 'fixture')?.validation_timeout_seconds;
  }).toBe(300);
});

test('Settings persists sections and saves memory configuration through the shared mock', async ({ page, request }) => {
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);

  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.evaluate(() => localStorage.clear());
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.getByRole('button', { name: 'Settings' }).click();

  const memorySection = page.getByRole('button', { name: /TDAI \/ memory/ });
  await expect(memorySection).toHaveAttribute('aria-expanded', 'false');
  await memorySection.click();
  await expect(memorySection).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('healthy', { exact: true })).toBeVisible();

  await page.getByLabel('Fixture (fixture)').uncheck();
  const globalPolicy = page.locator('fieldset').filter({ hasText: 'Global recall policy' });
  await globalPolicy.getByLabel('Character budget per turn').fill('1200');
  await globalPolicy.getByLabel('L1').uncheck();
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect.poll(async () => {
    const state = await request.get(`${MOCK_BASE_URL}/api/mock/state`).then((response) => response.json()) as {
      gateway: Record<string, unknown>;
    };
    return state.gateway;
  }).toMatchObject({
    enabled: true,
    disabledProfiles: ['fixture'],
    contextPolicy: { budgetChars: 1200, tiers: ['L0'] },
    contextPolicies: {}
  });

  // The open sidebar view is part of the URL, so Settings is still open.
  await page.reload({ waitUntil: 'domcontentloaded' });
  const reloadedMemorySection = page.getByRole('button', { name: /TDAI \/ memory/ });
  await expect(reloadedMemorySection).toHaveAttribute('aria-expanded', 'true');

  const skillSection = page.getByRole('button', { name: /Skill bank/ });
  await skillSection.click();
  await expect(page.getByText('gah-manager@1.0.0')).toBeVisible();
});

test('the routing editor sets the routine reviewer from an account picker', async ({ page, request }) => {
  test.setTimeout(120_000);
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);
  const calls: Array<{ action: string; body: Record<string, unknown> }> = [];
  await page.route('**/api/profiles/*/routing-candidates/*', async (route) => {
    calls.push({ action: route.request().url().split('/').pop() ?? '', body: route.request().postDataJSON() });
    await route.fulfill({ json: { success: true } });
  });
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.getByRole('button', { name: 'Profile', exact: true }).click();
  await page.getByRole('button', { name: /^Project:/ }).click();
  await page.getByRole('menuitemradio', { name: /^Fixture/ }).click();

  await page.getByRole('button', { name: 'Set reviewer' }).click();
  await page.getByLabel('Account').selectOption('backend:opencode');
  await page.getByLabel('Model').fill('tak-mistral-vibe/zai-glm-5-3');
  await page.getByRole('button', { name: 'Set reviewer' }).click();

  await expect.poll(() => calls.length).toBe(1);
  expect(calls[0]).toMatchObject({
    action: 'add',
    body: { list: 'routine', backend: 'opencode', model: 'tak-mistral-vibe/zai-glm-5-3', included_in_quota: true },
  });
});

test('the routing editor sends the named account, and remove and reorder name their list', async ({ page, request }) => {
  test.setTimeout(120_000);
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);
  const candidate = (model: string, priority: number) => ({
    backend: 'opencode', instance: null, model, quota_pool: null, priority, included_in_quota: true,
    marginal_cost_usd: null, quota_usage_percent: null, quota_days_remaining: null, requires_approval: false,
  });
  await page.route('**/api/config/effective?*', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({
      response,
      json: {
        ...body,
        backend_instances: [{
          backend_instance: 'tak-vibe', runner_kind: 'opencode', enabled: true, logical_backend: 'opencode',
          account_label: 'Tak Vibe', auth_source_label: null, quota_pool: null, supported_models: ['zai-glm-5-3'],
          executable_configured: true, isolated_state_configured: true,
        }],
        improve_candidates: [candidate('first', 200), candidate('second', 100)],
      },
    });
  });
  const calls: Array<{ action: string; body: Record<string, unknown> }> = [];
  await page.route('**/api/profiles/*/routing-candidates/*', async (route) => {
    calls.push({ action: route.request().url().split('/').pop() ?? '', body: route.request().postDataJSON() });
    await route.fulfill({ json: { success: true } });
  });
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.getByRole('button', { name: 'Profile', exact: true }).click();
  await page.getByRole('button', { name: /^Project:/ }).click();
  await page.getByRole('menuitemradio', { name: /^Fixture/ }).click();

  await page.getByRole('button', { name: 'Move opencode/second up' }).click();
  await expect.poll(() => calls.length).toBe(1);
  expect(calls[0]).toMatchObject({ action: 'move', body: { list: 'improve', from: 1, to: 0 } });

  await page.getByRole('button', { name: 'Remove opencode/first' }).click();
  await expect.poll(() => calls.length).toBe(2);
  expect(calls[1]).toMatchObject({ action: 'remove', body: { list: 'improve', index: 0 } });

  await page.getByRole('button', { name: '+ Add candidate' }).first().click();
  await page.getByLabel('Account').selectOption('instance:tak-vibe');
  await page.getByLabel('Model').fill('zai-glm-5-3');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await expect.poll(() => calls.length).toBe(3);
  expect(calls[2]).toMatchObject({
    action: 'add',
    body: { list: 'improve', backend: 'opencode', instance: 'tak-vibe', model: 'zai-glm-5-3', priority: 210 },
  });
});

test('profile management names fields and safely contains delete confirmation focus', async ({ page, request }) => {
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);
  await page.goto('/');
  await page.getByRole('button', { name: 'Profile', exact: true }).click();
  await page.getByRole('button', { name: 'Add Profile', exact: true }).click();
  for (const label of ['Display Name *', 'Profile Name (ID) *', 'Provider *', 'Repository *', 'Local Path *', 'Artifact Root *', 'Default Branch']) {
    await expect(page.getByLabel(label, { exact: true })).toBeVisible();
  }
  await page.getByLabel('Display Name *', { exact: true }).fill('Keyboard project');
  await expect(page.getByLabel('Display Name *', { exact: true })).toHaveValue('Keyboard project');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  const opener = page.getByRole('button', { name: 'Delete profile', exact: true }).first();
  await opener.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Delete Profile', exact: true });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
  await expect(dialog).toHaveAccessibleDescription(/This action cannot be undone/);
  await page.keyboard.press('Tab');
  await expect(dialog.getByRole('button', { name: 'Delete Profile', exact: true })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(opener).not.toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
  await opener.click();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
});
