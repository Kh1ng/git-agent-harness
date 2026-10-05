import { expect, test } from '@playwright/test';
import { formatUntil, parseWindow, projectedPercent, subscriptionUsage } from '../../src/lib/subscriptionUsage.js';

// Subscription usage, in the style of Claude's usage popover and Poracode's
// provider cards: a ring per subscription in the navbar, a popover with
// each window's bar, reset time and pace projection, and collapsible
// cards on Usage > Quota.

const NOW = Date.parse('2026-10-04T23:00:00Z');
const candidates = [
  { backend: 'claude', backend_instance: 'claude', provider: 'anthropic', model: 'sonnet', modes: ['improve'], configured: false, eligible_now: true, usage: { actual_cost_usd: null },
    quota_observations: [
      { backend: 'claude', quota_window: 'weekly', quota_used_percent: 14, quota_remaining_percent: 86, quota_reset_at: '2026-10-07T13:00:00Z', observed_at: '2026-10-04T22:58:00Z' },
      { backend: 'claude', quota_window: '5-hour', quota_used_percent: 66, quota_remaining_percent: 34, quota_reset_at: '2026-10-04T23:20:00Z', observed_at: '2026-10-04T22:58:00Z' }
    ] },
  { backend: 'codex', backend_instance: 'codex', provider: 'openai', model: 'gpt-6-sol', modes: ['improve'], configured: false, eligible_now: true, usage: { actual_cost_usd: 8.1 },
    quota_observations: [{ backend: 'codex', quota_window: '10080m', quota_remaining_percent: 94, quota_reset_at: '2026-10-11T08:06:15Z' }] },
  { backend: 'vibe', backend_instance: 'vibe', provider: 'z-ai', model: 'glm-5-3', modes: ['improve'], configured: false, eligible_now: false, reason: 'quota exhausted', unavailable_until: '2026-10-05T01:00:00Z', usage: {} }
];

// Routes that fetch from the fixture server must not outlive the test.
test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test('windows are named and sized from the provider names, and projected to reset', () => {
  expect(parseWindow('5-hour')).toEqual({ label: 'Session (5h)', windowMs: 5 * 3_600_000 });
  expect(parseWindow('300m')).toEqual({ label: 'Session (5h)', windowMs: 5 * 3_600_000 });
  expect(parseWindow('10080m')).toEqual({ label: 'Weekly', windowMs: 7 * 86_400_000 });
  expect(parseWindow('weekly').label).toBe('Weekly');
  expect(parseWindow('AGY individual quota')).toEqual({ label: 'AGY individual quota', windowMs: null });

  const [claude, codex, vibe] = subscriptionUsage({ candidates } as never);
  expect(claude.providerLabel).toBe('Anthropic');
  expect(claude.windows.map((window) => [window.label, window.usedPercent])).toEqual([['Session (5h)', 66], ['Weekly', 14]]);
  expect(claude.tightest?.label).toBe('Session (5h)');
  expect(codex.windows[0].usedPercent).toBe(6);
  expect(codex.costUsd).toBe(8.1);
  expect(vibe.windows).toEqual([]);
  expect(vibe.eligible).toBe(false);

  // 66% used with 20 minutes of a 5-hour window left: on pace for about 71%.
  expect(projectedPercent(claude.windows[0], NOW)).toBe(71);
  // Just started a window: no projection yet.
  expect(projectedPercent({ ...claude.windows[0], resetAt: '2026-10-05T03:55:00Z' }, NOW)).toBeNull();
  expect(formatUntil('2026-10-04T23:20:00Z', NOW)).toBe('20 min');
  expect(formatUntil('2026-10-07T13:00:00Z', NOW)).toBe('2d 14h');
  expect(formatUntil('2026-10-04T22:00:00Z', NOW)).toBe('now');
});

test('the navbar shows a ring per subscription and opens its windows; Quota lists collapsible cards', async ({ page }) => {
  const soon = new Date(Date.now() + 20 * 60_000).toISOString();
  const week = new Date(Date.now() + 62 * 3_600_000).toISOString();
  await page.route('**/api/quota?*', async (route) => {
    const snapshot = await (await route.fetch()).json();
    snapshot.candidates = candidates.map((candidate) => ({ ...candidate, quota_observations: candidate.quota_observations?.map((o) => ({ ...o, quota_reset_at: o.quota_window === '5-hour' ? soon : week })) }));
    await route.fulfill({ json: snapshot });
  });
  await page.goto('/?page=overview&profile=fixture');
  const chips = page.getByRole('group', { name: 'Subscription usage' });
  await expect(chips.getByRole('button')).toHaveCount(3);
  const claude = chips.getByRole('button', { name: /Anthropic usage: 66% of session \(5h\) used/ });
  await claude.hover();
  const popover = page.getByRole('dialog', { name: 'Anthropic usage' });
  await expect(popover).toContainText('66% of session (5h) used. Resets in 20 min.');
  await expect(popover).toContainText('That is the tightest limit right now.');
  await expect(popover.getByRole('progressbar', { name: 'Session (5h)' })).toHaveAttribute('aria-valuenow', '66');
  await expect(popover.getByRole('progressbar', { name: 'Weekly' })).toHaveAttribute('aria-valuenow', '14');
  await expect(popover).toContainText('≈71% by reset');
  await popover.getByRole('button', { name: 'See all subscriptions' }).click();
  await expect(page.getByRole('heading', { name: 'Quota management', exact: true })).toBeVisible();

  const cards = page.getByRole('region', { name: 'Subscriptions' });
  const codex = cards.getByRole('article', { name: 'OpenAI subscription' });
  await expect(codex).toContainText('Weekly6%');
  await codex.getByRole('button', { name: /OpenAI/ }).click();
  await expect(codex.getByRole('progressbar', { name: 'Weekly' })).toHaveAttribute('aria-valuenow', '6');
  await expect(codex).toContainText('spend $8.10 this period');
  await expect(cards.getByRole('article', { name: 'Z.ai subscription' })).toContainText('quota exhausted');
});
