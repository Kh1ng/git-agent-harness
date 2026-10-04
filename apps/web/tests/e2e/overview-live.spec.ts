import { expect, test } from '@playwright/test';
import { buildLiveRows, formatDuration } from '../../src/components/LiveAgentsCard.js';

// Overview's Live card: one row per agent account, busy rows first, with
// the job, how long it has run, its claim, and the files its last attempt
// changed. Modelled on the Gauntlet coord dashboard's live panel.

const instance = (id: string, backend: string, extra: Record<string, unknown> = {}) => ({
  backend_instance: id, runner_kind: backend, logical_backend: backend, enabled: true, account_label: null,
  auth_source_label: null, quota_pool: null, supported_models: ['model-1'], executable_configured: true,
  isolated_state_configured: true, ...extra
});

test('rows derive their state from sessions, instance health, quota and claims', () => {
  const now = Date.parse('2026-10-04T12:00:00Z');
  const rows = buildLiveRows({
    instances: [
      instance('claude-main', 'claude', { account_label: 'Work account' }),
      instance('codex-work', 'codex', { enabled: false }),
      instance('agy-second', 'agy', { auth_ready: false, resolution_error: 'login expired' }),
      instance('vibe', 'vibe'),
      instance('gemini', 'gemini')
    ] as never,
    sessions: [
      { id: 's1', providerKind: 'github', instanceId: 'claude-main', status: 'running', backend: 'claude', model: 'opus', target: '#946', mode: 'improve', startedAt: '2026-10-04T11:55:00Z' },
      { id: 's2', providerKind: 'github', instanceId: 'other', status: 'running', backend: 'hermes', target: '#950', mode: 'review', startedAt: '2026-10-04T11:59:30Z' },
      { id: 's3', providerKind: 'github', instanceId: 'vibe', status: 'stopped', backend: 'vibe', target: '#900' }
    ] as never,
    controllerRuns: [{ run_id: 'r1', profile: 'gah', work_id: '#951', started_at: '2026-10-04T11:58:00Z', finished_at: null, action: 'dispatch: merge #951', status: 'running', outcome: null }],
    claims: [{ work_id: '#946', pid: 1, scope: 'improve', hostname: 'box', claimed_at: '2026-10-04T11:55:10Z', age_seconds: 290 }],
    candidates: [{ backend: 'gemini', model: null, modes: [], configured: true, eligible_now: false, reason: 'quota exhausted', unavailable_until: '2026-10-04T13:00:00Z', usage: {} as never }],
    now
  });
  expect(rows.map((row) => [row.name, row.state, row.job])).toEqual([
    ['Work account', 'working', '#946'],
    ['other', 'gates', '#950'],
    ['controller', 'working', '#951'],
    ['codex-work', 'halted', null],
    ['agy-second', 'down', null],
    ['gemini', 'paused', null],
    ['vibe', 'idle', null]
  ]);
  expect(rows[0].claimAgeSeconds).toBe(290);
  expect(rows[4].reason).toBe('login expired');
  expect(rows[5].resumes).toBe('2026-10-04T13:00:00Z');
  expect(formatDuration(now - Date.parse(rows[0].since!))).toBe('5m 00s');
  expect(formatDuration(3_900_000)).toBe('1h 05m');
  expect(formatDuration(-5)).toBe('0s');
});

test('Overview shows each agent account with its job, elapsed time, claim and files changed', async ({ page }) => {
  const startedAt = new Date(Date.now() - 5 * 60_000).toISOString();
  await page.route('**/api/backend-instances**', (route) => route.fulfill({ json: { profile: 'fixture', backend_instances: [
    instance('claude-main', 'claude', { account_label: 'Work account' }),
    instance('codex-work', 'codex', { enabled: false })
  ] } }));
  await page.route('**/api/work/**', (route) => route.fulfill({ json: [
    { timestamp: new Date(Date.now() - 60_000).toISOString(), work_id: '#946', files_changed: 3 }
  ] }));
  await page.route('**/api/status**', async (route) => {
    const snapshot = await (await route.fetch()).json();
    snapshot.active_claims = [{ work_id: '#946', pid: 1, scope: 'improve', hostname: 'box', claimed_at: startedAt, age_seconds: 290 }];
    await route.fulfill({ json: snapshot });
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify({
      type: 'server.welcome', serverVersion: '0.0.0-test', serverProviderCatalog: { providers: [] }, providers: {},
      sessions: [{ id: 's1', providerKind: 'github', instanceId: 'claude-main', status: 'running', backend: 'claude', model: 'opus', target: '#946', mode: 'improve', startedAt }]
    }));
    ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
  });
  await page.goto('/?page=overview&profile=fixture');
  const live = page.getByRole('region', { name: 'Live: what each agent is doing now' });
  const agents = live.getByRole('list', { name: 'Agents' }).getByRole('listitem');
  await expect(agents).toHaveCount(2);
  await expect(agents.nth(0)).toContainText('Work account');
  await expect(agents.nth(0)).toContainText(/improve on #946 for 5m \d\ds/);
  await expect(agents.nth(0)).toContainText('3 files changed · claimed 4m 50s ago · last attempt 1m');
  await expect(agents.nth(0).getByRole('img', { name: 'working' })).toBeVisible();
  await expect(agents.nth(1)).toContainText('codex-work');
  await expect(agents.nth(1)).toContainText('disabled, routing skips it');
  await expect(live).toContainText('1 busy of 2');
  // The timer ticks while a row is busy.
  const before = await agents.nth(0).textContent();
  await expect.poll(async () => (await agents.nth(0).textContent()) !== before, { timeout: 5000 }).toBe(true);
});
