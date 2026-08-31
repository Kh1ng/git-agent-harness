import { expect, test } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

test('Settings exposes validation timeout and persists profile updates in the shared mock', async ({ page, request }) => {
  test.setTimeout(120_000);
  await request.post(`${MOCK_BASE_URL}/api/mock/reset`);

  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.evaluate(() => localStorage.clear());
  await page.reload({ waitUntil: 'domcontentloaded' });
  const settingsButton = page.getByRole('button', { name: 'Settings' });
  await expect(settingsButton).toBeVisible({ timeout: 60_000 });
  await settingsButton.click();
  await page.getByText('Factory / profile management', { exact: true }).click();

  const validationTimeoutInput = page
    .getByText('Validation command timeout (seconds)')
    .locator('..')
    .locator('input');
  const profileSelect = page
    .locator('section')
    .filter({ hasText: 'Which configured GAH repo' })
    .getByRole('combobox');
  await profileSelect.selectOption('fixture');
  await expect(page.getByText(/Per-profile loop behavior for/)).toBeVisible();

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

  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.getByRole('button', { name: 'Settings' }).click();
  const reloadedMemorySection = page.getByRole('button', { name: /TDAI \/ memory/ });
  await expect(reloadedMemorySection).toHaveAttribute('aria-expanded', 'true');

  const skillSection = page.getByRole('button', { name: /Skill bank/ });
  await skillSection.click();
  await expect(page.getByText('gah-manager@1.0.0')).toBeVisible();
});
