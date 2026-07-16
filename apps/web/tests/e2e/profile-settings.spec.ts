import { expect, test } from '@playwright/test';

const BASE_PROFILE = {
  name: 'test-repo',
  display_name: 'Test Repo',
  provider: 'github',
  repo: 'owner/test-repo',
  local_path: '/tmp/test-repo',
  web_url: 'https://github.com/owner/test-repo',
  max_parallel_workers: 1,
  validation_timeout_seconds: 300,
  manager_wake_autonomy: 'off'
};

test('Settings exposes validation timeout and sends it to profile update API', async ({ page }) => {
  test.setTimeout(120_000);
  let updatePayload: Record<string, unknown> | null = null;

  await page.route('**/api/**', (route) => {
    const request = route.request();
    const method = request.method();
    const url = new URL(request.url());

    if (url.pathname === '/api/profiles' && method === 'GET') {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([BASE_PROFILE]),
      });
      return;
    }

    if (url.pathname === '/api/config' && method === 'GET') {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ current_manager: null }),
      });
      return;
    }

    if (url.pathname === '/api/profiles/test-repo' && method === 'PATCH') {
      updatePayload = request.postDataJSON() as Record<string, unknown>;
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          message: 'Profile updated'
        }),
      });
      return;
    }

    route.continue();
  });

  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  const settingsButton = page.getByRole('button', { name: 'Settings' });
  await expect(settingsButton).toBeVisible({ timeout: 60_000 });
  await settingsButton.click();

  const validationTimeoutInput = page
    .getByText('Validation command timeout (seconds)')
    .locator('..')
    .locator('input');
  const profileSelect = page.getByRole('combobox');
  await profileSelect.selectOption('test-repo');
  await expect(page.getByText(/Per-profile loop behavior for/)).toBeVisible();

  await expect(validationTimeoutInput).toBeVisible();
  await expect(validationTimeoutInput).toHaveValue('300');
  await expect(page.getByText(/validation command timeout/i)).toBeVisible();
  await expect(page.getByText(/backend idle timeouts/i)).toBeVisible();

  await validationTimeoutInput.fill('900');
  await page.getByRole('button', { name: 'Save dispatch settings' }).click();

  await expect
    .poll(() => updatePayload)
    .not.toBeNull();

  expect(updatePayload).toMatchObject({
    validation_timeout_seconds: 900
  });

  updatePayload = null;
  await validationTimeoutInput.fill('');
  await page.getByRole('button', { name: 'Save dispatch settings' }).click();

  await expect
    .poll(() => updatePayload)
    .not.toBeNull();

  expect(updatePayload).toMatchObject({
    clear: ['validation_timeout_seconds']
  });
  expect(updatePayload).not.toHaveProperty('validation_timeout_seconds');
});
