import { expect, test } from '@playwright/test';

const now = new Date().toISOString();
const resetAt = new Date(Date.now() + 3_600_000).toISOString();

const usage = {
  entries: 1,
  attempts: 1,
  validation_pass: 1,
  success_rate: 1,
  total_tokens: 100,
  requests_count: 1,
  actual_cost_usd: null,
  estimated_cost_usd: null
};

test('quota windows compare exact percentages and distinguish missing, stale, and unavailable data', async ({ page }) => {
  await page.route('**/api/quota**', route => route.fulfill({
    json: {
      schema_version: 2,
      generated_at: now,
      freshness: { quota_observed_at: now },
      quota_checks: [],
      profile: { profile: 'fixture', display_name: 'Fixture', repo_id: 'fixture', provider: 'github', local_path: '/fixture', default_target_branch: 'main' },
      since: '7d',
      usage,
      candidates: [
        {
          modes: ['fix'], backend: 'codex', model: 'gpt-5.3', quota_pool: 'subscription', configured: true, eligible_now: true, observed_at: now, usage,
          quota_observations: [{ backend: 'codex', model: 'gpt-5.3', quota_window: '5-hour', quota_used_percent: 40, quota_remaining_percent: 60, quota_reset_at: resetAt, observed_at: now, usage_source: 'subscription' }]
        },
        {
          modes: ['review'], backend: 'claude', model: 'opus', quota_pool: 'subscription', configured: true, eligible_now: true, observed_at: now, usage,
          quota_observations: [{ backend: 'claude', model: 'opus', quota_window: 'weekly', quota_used_percent: null, quota_remaining_percent: 25, quota_reset_at: resetAt, observed_at: now, usage_source: 'subscription' }]
        },
        {
          modes: ['fix'], backend: 'vibe', model: null, quota_pool: 'metered', configured: true, eligible_now: false, reason: 'Account check failed', observed_at: '2000-01-01T00:00:00Z', usage,
          quota_observations: [{ backend: 'vibe', quota_window: 'monthly', quota_used_percent: null, quota_remaining_percent: null, quota_reset_at: null, observed_at: '2000-01-01T00:00:00Z', usage_source: 'provider' }]
        }
      ]
    }
  }));

  await page.goto('/');
  await page.getByRole('button', { name: 'Quota', exact: true }).click();

  const codex = page.locator('.card-padded').filter({ hasText: 'codex / subscription / gpt-5.3' });
  const claude = page.locator('.card-padded').filter({ hasText: 'claude / subscription / opus' });
  const vibe = page.locator('.card-padded').filter({ hasText: 'vibe / metered' });

  await expect(codex.getByRole('progressbar', { name: '5-hour · gpt-5.3: 40% used, 60% remaining' })).toBeVisible();
  await expect(claude.getByRole('progressbar', { name: 'weekly · opus: 75% used, 25% remaining' })).toBeVisible();
  await expect(codex.getByText(/Resets in (59m|1h 0m)/)).toBeVisible();
  await expect(vibe.getByText('Unavailable', { exact: true })).toBeVisible();
  await expect(vibe.getByText('Stale', { exact: true })).toHaveCount(2);
  await expect(vibe.getByText('No usage percentage available', { exact: true })).toBeVisible();
  await expect(vibe.getByRole('progressbar')).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
});
