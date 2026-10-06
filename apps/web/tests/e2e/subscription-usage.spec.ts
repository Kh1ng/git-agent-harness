import { expect, test } from '@playwright/test';
import { busySubscriptionIds, formatUntil, parseWindow, projectedPercent, subscriptionUsage } from '../../src/lib/subscriptionUsage.js';

// Subscription usage, in the style of Claude's usage popover and Poracode's
// provider cards: a ring per subscription in the navbar, a popover with
// each window's bar, reset time and pace projection, and collapsible
// cards on Usage > Quota.

const NOW = Date.parse('2026-10-04T23:00:00Z');
const candidates = [
  { backend: 'claude', backend_instance: 'claude', provider: 'anthropic', model: 'sonnet', modes: ['improve'], configured: false, eligible_now: true, usage: { actual_cost_usd: null },
    quota_observations: [
      { backend: 'claude', quota_window: 'weekly', quota_remaining_percent: 86, quota_reset_at: '2026-10-07T13:00:00Z', observed_at: '2026-10-04T22:58:00Z' },
      { backend: 'claude', quota_window: '5-hour', quota_remaining_percent: 34, quota_reset_at: '2026-10-04T23:20:00Z', observed_at: '2026-10-04T22:58:00Z' }
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

  // A running session names its backend; a loop job is credited through the latest dispatch.
  const subscriptions = [claude, codex, vibe];
  expect([...busySubscriptionIds({ subscriptions, sessions: [{ id: 's', providerKind: 'github', instanceId: 'claude', status: 'running', backend: 'claude' }] as never, controllerRuns: [], claims: [], recentLedger: null })]).toEqual(['claude']);
  expect([...busySubscriptionIds({ subscriptions, sessions: [], controllerRuns: [{ run_id: 'r', profile: 'gah', work_id: '#1', started_at: '', finished_at: null, action: 'fix', status: 'running', outcome: null }],
    claims: [], recentLedger: { most_recent_work_id: '#1', most_recent_effective_backend: 'codex' } as never })]).toEqual(['codex']);
  expect(busySubscriptionIds({ subscriptions, sessions: [], controllerRuns: [], claims: [], recentLedger: { most_recent_work_id: '#1', most_recent_effective_backend: 'codex' } as never }).size).toBe(0);
  // A factory agent process is a subscription at work, whatever the ledger says.
  expect([...busySubscriptionIds({ subscriptions, sessions: [], controllerRuns: [], claims: [], recentLedger: null, factoryAgents: [{ pid: 1, tool: 'codex', cwd: '/w', started_at: null }] })]).toEqual(['codex']);
  // Two subscriptions behind one CLI: the model the process runs picks the one that is busy.
  const pools = [
    { ...subscriptions[0], id: 'agy:google-native', backend: 'agy', model: 'Gemini Pro' },
    { ...subscriptions[0], id: 'agy:external', backend: 'agy', model: 'Claude Sonnet' }
  ];
  const agyAgent = (model?: string) => ({ pid: 1, tool: 'agy', cwd: '/w', started_at: null, ...(model ? { model } : {}) });
  expect([...busySubscriptionIds({ subscriptions: pools, sessions: [], controllerRuns: [], claims: [], recentLedger: null, factoryAgents: [agyAgent('Claude Sonnet')] })]).toEqual(['agy:external']);
  expect([...busySubscriptionIds({ subscriptions: pools, sessions: [], controllerRuns: [], claims: [], recentLedger: null, factoryAgents: [agyAgent()] })]).toEqual(['agy:google-native']);
});

test('one ring per account: pools are windows of it, router aliases are not accounts (#1411)', () => {
  const weekly = (backend: string, instance: string, used: number) => ({
    backend, provider: 'antigravity', backend_instance: instance, quota_pool: instance, checked_at: '', status: 'data',
    quota_observations: [{ backend, quota_window: 'weekly', quota_used_percent: used, observed_at: '2026-10-05T19:48:00Z' }]
  });
  const usage = subscriptionUsage({
    candidates: [{ backend: 'agy', backend_instance: 'agy:external', provider: 'antigravity', model: 'Claude Sonnet 4.6 (Thinking)', modes: [], configured: true, eligible_now: true, usage: {} }],
    quota_checks: [
      weekly('agy', 'agy:external', 40), weekly('agy', 'agy:google-native', 100),
      weekly('agy-second', 'agy-second:external', 0), weekly('agy-second', 'agy-second:google-native', 70),
      { ...weekly('agy', 'cli-router:2dbd806c', 0), status: 'no_data', quota_observations: [] }
    ]
  } as never);
  expect(usage.map((account) => account.id)).toEqual(['agy', 'agy-second']);
  expect(usage[0].windows.map((window) => [window.label, window.usedPercent])).toEqual([['External models · Weekly', 40], ['Gemini · Weekly', 100]]);
  expect(usage[0].tightest?.label).toBe('Gemini · Weekly');
  expect(usage[1].windows.map((window) => window.label)).toEqual(['External models · Weekly', 'Gemini · Weekly']);
});

test('the navbar shows a ring per subscription and opens its windows; Quota lists collapsible cards', async ({ page }) => {
  const soon = new Date(Date.now() + 20 * 60_000).toISOString();
  const week = new Date(Date.now() + 62 * 3_600_000).toISOString();
  await page.route('**/api/quota?*', async (route) => {
    const snapshot = await (await route.fetch()).json();
    snapshot.quota_checks = [];
    snapshot.candidates = candidates.map((candidate) => ({ ...candidate, quota_observations: candidate.quota_observations?.map((o) => ({ ...o, quota_reset_at: o.quota_window === '5-hour' ? soon : week })) }));
    await route.fulfill({ json: snapshot });
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify({
      type: 'server.welcome', serverVersion: '0.0.0-test', serverProviderCatalog: { providers: [] }, providers: {},
      sessions: [{ id: 's1', providerKind: 'github', instanceId: 'codex', status: 'running', backend: 'codex', target: '#946', mode: 'improve', startedAt: new Date().toISOString() }]
    }));
    ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
  });
  await page.goto('/?page=overview&profile=fixture');
  const chips = page.getByRole('group', { name: 'Subscription usage' });
  await expect(chips.getByRole('button')).toHaveCount(3);
  // The card is the label: no tooltip duplicates it. A working subscription's letter shimmers.
  const claude = chips.getByRole('button', { name: /Anthropic claude usage: 66% of session \(5h\) used/ });
  await expect(claude).not.toHaveAttribute('title', /.+/);
  await expect(chips.getByRole('button', { name: /OpenAI codex usage/ }).locator('[data-working]')).toHaveText('O');
  await expect(claude.locator('[data-working]')).toHaveCount(0);
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
