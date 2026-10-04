import { expect, test } from '@playwright/test';
import { buildLiveRows, formatDuration, liveAccounts } from '../../src/components/LiveAgentsCard.js';

// Overview's Live card: one row per agent account, busy rows first, with
// the job, how long it has run, its claim, and the files its last attempt
// changed. Modelled on the Gauntlet coord dashboard's live panel.

const instance = (id: string, backend: string, extra: Record<string, unknown> = {}) => ({
  backend_instance: id, runner_kind: backend, logical_backend: backend, enabled: true, account_label: null,
  auth_source_label: null, quota_pool: null, supported_models: ['model-1'], executable_configured: true,
  isolated_state_configured: true, ...extra
});

test('rows derive their state from sessions, runs, claims, instance health and quota', () => {
  const now = Date.parse('2026-10-04T12:00:00Z');
  const candidate = (backend: string, extra: Record<string, unknown> = {}) => ({ backend, backend_instance: backend, model: 'm', modes: ['improve'], configured: true, eligible_now: true, usage: {} as never, ...extra });
  // Declared instances come first; candidates add the accounts routing knows and refine the rest.
  const accounts = liveAccounts([
    instance('claude-main', 'claude', { account_label: 'Work account' }),
    instance('codex-work', 'codex', { enabled: false }),
    instance('agy-second', 'agy', { auth_ready: false, resolution_error: 'login expired' })
  ] as never, [
    candidate('claude', { backend_instance: null }),
    candidate('vibe'),
    candidate('gemini', { eligible_now: false, reason: 'quota exhausted', unavailable_until: '2026-10-04T13:00:00Z' }),
    candidate('hermes', { eligible_now: false, reason: 'no login' })
  ] as never, now);
  expect(accounts.map((account) => account.id)).toEqual(['claude-main', 'codex-work', 'agy-second', 'vibe', 'gemini', 'hermes']);

  const rows = buildLiveRows({
    accounts,
    sessions: [
      { id: 's1', providerKind: 'github', instanceId: 'claude-main', status: 'running', backend: 'claude', model: 'opus', target: '#946', mode: 'improve', startedAt: '2026-10-04T11:55:00Z' },
      { id: 's2', providerKind: 'github', instanceId: 'other', status: 'running', backend: 'other', target: '#950', mode: 'review', startedAt: '2026-10-04T11:59:30Z' },
      { id: 's3', providerKind: 'github', instanceId: 'vibe', status: 'stopped', backend: 'vibe', target: '#900' }
    ] as never,
    controllerRuns: [
      // A loop job: its ledger entry names the backend, so it lands on that account.
      { run_id: 'r1', profile: 'gah', work_id: '#951', started_at: '2026-10-04T11:58:00Z', finished_at: null, action: 'fix_existing: gah/gah-1', status: 'running', outcome: null },
      { run_id: 'r2', profile: 'gah', work_id: '#952', started_at: '2026-10-04T11:57:00Z', finished_at: null, action: 'merge: gah/gah-2', status: 'running', outcome: null },
      { run_id: 'r0', profile: 'gah', work_id: '#940', started_at: '2026-10-04T11:50:00Z', finished_at: '2026-10-04T11:51:00Z', action: 'fix_existing: x', status: 'failed', outcome: 'deferred' }
    ],
    claims: [
      { work_id: '#946', pid: 1, scope: 'improve', hostname: 'box', claimed_at: '2026-10-04T11:55:10Z', age_seconds: 290 },
      { work_id: '#953', pid: 2, scope: 'improve', hostname: 'box', claimed_at: '2026-10-04T11:59:00Z', age_seconds: 60 }
    ],
    ledgers: { '#951': { effective_backend: 'vibe', mode: 'fix', files_changed: 2 } as never, '#953': null }
  });
  expect(rows.map((row) => [row.name, row.state, row.job, row.mode])).toEqual([
    ['Work account', 'working', '#946', 'improve'],
    ['vibe', 'working', '#951', 'fix'],
    ['other', 'gates', '#950', 'review'],
    ['controller', 'gates', '#952', 'merge'],
    ['controller', 'working', '#953', 'improve'],
    ['codex-work', 'halted', null, null],
    ['agy-second', 'down', null, null],
    ['gemini', 'paused', null, null],
    ['hermes', 'down', null, null]
  ]);
  expect(rows[0].claimAgeSeconds).toBe(290);
  expect(rows[4].since).toBe('2026-10-04T11:59:00Z');
  expect(rows[6].reason).toBe('login expired');
  expect(rows[7].resumes).toBe('2026-10-04T13:00:00Z');
  expect(rows[8].reason).toBe('no login');
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
