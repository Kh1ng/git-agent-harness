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

const EFFECTIVE_CONFIG = {
  profile: 'test-repo',
  merge_policy: 'auto',
  max_fix_attempts_per_mr: 2,
  max_implementation_failures_per_ticket: 8,
  max_review_cycles_per_ticket: 3,
  max_paid_reviews_per_ticket: 3,
  pm_candidates: [],
  improve_candidates: [{ backend: 'opencode', model: null, quota_pool: null, priority: 1, included_in_quota: true, marginal_cost_usd: null, quota_usage_percent: null, quota_days_remaining: null, requires_approval: false }],
  review_candidates: [],
  task_routing_rules: [{ modes: ['improve'], task_classes: [], difficulties: ['easy'], risks: [], candidates: [{ backend: 'codex', model: 'gpt-5.3-codex-spark', quota_pool: 'codex', priority: 1, included_in_quota: true, marginal_cost_usd: null, quota_usage_percent: null, quota_days_remaining: null, requires_approval: false }] }],
  routine_reviewer: null,
  escalatory_reviewers: [],
  context: {
    global: { enabled: true, soft_limit_tokens: 80_000, hard_limit_tokens: 150_000, compact_after_tool_calls: 20, fresh_context_on_review: true, fresh_context_on_fix: true, include_full_git_history: false, include_full_worker_transcript_in_review: false, recent_history_tokens: 20_000 },
    profile_override: null,
    effective_by_backend: []
  },
  notifications: { configured: true, transport: 'telegram', manager_wake_autonomy: 'review_only', env_file_configured: true, env_file_prod_configured: false }
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

    if (url.pathname === '/api/config/effective' && method === 'GET') {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(EFFECTIVE_CONFIG),
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
  await page.getByText('Factory / profile management', { exact: true }).click();

  const validationTimeoutInput = page
    .getByText('Validation command timeout (seconds)')
    .locator('..')
    .locator('input');
  const profileSelect = page
    .locator('section')
    .filter({ hasText: 'Which configured GAH repo' })
    .getByRole('combobox');
  await profileSelect.selectOption('test-repo');
  await expect(page.getByText(/Per-profile loop behavior for/)).toBeVisible();

  await expect(validationTimeoutInput).toBeVisible();
  await expect(validationTimeoutInput).toHaveValue('300');
  await expect(page.getByText(/validation command timeout/i)).toBeVisible();
  await expect(page.getByText(/backend idle timeouts/i)).toBeVisible();
  await expect(page.getByText('opencode/unknown')).toBeVisible();
  await expect(page.getByText(/codex\/gpt-5\.3-codex-spark/)).toBeVisible();
  await expect(page.getByText(/Target: telegram/)).toBeVisible();
  await expect(page.getByText(/prod env: not configured/)).toBeVisible();

  await validationTimeoutInput.fill('0');
  await expect(page.getByRole('alert')).toContainText(/whole number of seconds greater than zero/i);
  await expect(page.getByRole('button', { name: 'Save dispatch settings' })).toBeDisabled();
  expect(updatePayload).toBeNull();

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

test('Settings persists sections and saves the full memory gateway configuration', async ({ page }) => {
  let gatewayPayload: Record<string, unknown> | null = null;
  const gatewaySettings = {
    url: 'http://127.0.0.1:8420',
    apiKeyConfigured: true,
    enabled: true,
    disabledProfiles: [],
    contextPolicy: {},
    contextPolicies: {},
    degraded: { degraded: false, lastError: null, lastFailedAt: null, lastOkAt: 1_700_000_000_000 },
    tailscaleIPv4: '100.64.0.1'
  };

  await page.route('**/api/**', (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === '/api/profiles' && request.method() === 'GET') {
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([BASE_PROFILE]) });
      return;
    }
    if (url.pathname === '/api/settings/gateway' && request.method() === 'GET') {
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(gatewaySettings) });
      return;
    }
    if (url.pathname === '/api/settings/gateway' && request.method() === 'PUT') {
      gatewayPayload = request.postDataJSON() as Record<string, unknown>;
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ...gatewaySettings, ...gatewayPayload }) });
      return;
    }
    if (url.pathname === '/api/skills') {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ skills: [{ id: 'test-skill', version: '1.0.0', displayName: 'Test skill', description: 'A useful test skill.', backends: ['codex'], source: 'test', bound: true }] })
      });
      return;
    }
    route.continue();
  });

  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.evaluate(() => localStorage.clear());
  await page.getByRole('button', { name: 'Settings' }).click();

  const memorySection = page.getByRole('button', { name: /TDAI \/ memory/ });
  await expect(memorySection).toHaveAttribute('aria-expanded', 'false');
  await memorySection.click();
  await expect(memorySection).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('healthy', { exact: true })).toBeVisible();

  await page.getByLabel('Test Repo (test-repo)').uncheck();
  const globalPolicy = page.locator('fieldset').filter({ hasText: 'Global recall policy' });
  await globalPolicy.getByLabel('Character budget per turn').fill('1200');
  await globalPolicy.getByLabel('L0').check();
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect.poll(() => gatewayPayload).not.toBeNull();
  expect(gatewayPayload).toMatchObject({
    enabled: true,
    disabledProfiles: ['test-repo'],
    contextPolicy: { budgetChars: 1200, tiers: ['L0'] },
    contextPolicies: {}
  });

  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.getByRole('button', { name: 'Settings' }).click();
  const reloadedMemorySection = page.getByRole('button', { name: /TDAI \/ memory/ });
  await expect(reloadedMemorySection).toHaveAttribute('aria-expanded', 'true');

  const skillSection = page.getByRole('button', { name: /Skill bank/ });
  await skillSection.click();
  await expect(page.getByText('test-skill@1.0.0')).toBeVisible();
});
