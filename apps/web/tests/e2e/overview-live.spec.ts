import { expect, test } from '@playwright/test';
import { agentDisplayName, buildLiveRows, formatDuration, liveAccounts } from '../../src/components/LiveAgentsCard.js';

// Overview's Live card: one row per agent account, busy rows first, with
// the job, how long it has run, its claim, and the files its last attempt
// changed. Modelled on the Gauntlet coord dashboard's live panel.

const instance = (id: string, backend: string, extra: Record<string, unknown> = {}) => ({
  backend_instance: id, runner_kind: backend, logical_backend: backend, enabled: true, account_label: null,
  auth_source_label: null, quota_pool: null, supported_models: ['model-1'], executable_configured: true,
  isolated_state_configured: true, ...extra
});

// Routes that fetch from the fixture server must not outlive the test.
test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

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

  // A running dispatch has no ledger entry: the factory's agent processes name its backend.
  const idle = liveAccounts([], [candidate('codex'), candidate('claude')] as never, now);
  const named = buildLiveRows({
    accounts: idle, sessions: [], ledgers: {},
    controllerRuns: [
      { run_id: 'a', profile: 'gah', work_id: '#1380', started_at: '2026-10-04T11:54:43Z', finished_at: null, action: 'dispatch_ticket: 1380', status: 'running', outcome: null },
      { run_id: 'b', profile: 'gah', work_id: '#1381', started_at: '2026-10-04T11:54:47Z', finished_at: null, action: 'dispatch_ticket: 1381', status: 'running', outcome: null }
    ],
    claims: [],
    factoryAgents: [
      { pid: 2, tool: 'claude', cwd: '/w/b', started_at: '2026-10-04T11:55:14Z' },
      { pid: 1, tool: 'codex', cwd: '/w/a', started_at: '2026-10-04T11:55:13Z', model: 'gpt-6-luna' }
    ]
  });
  // The process's own --model wins; otherwise the account's routing model is shown.
  expect(named.map((row) => [row.name, row.state, row.job, row.model])).toEqual([['codex', 'working', '#1380', 'gpt-6-luna'], ['claude', 'working', '#1381', 'm']]);
  // One process per job: it outranks a ledger entry left by an earlier attempt on another backend.
  const retried = buildLiveRows({ accounts: idle, sessions: [], claims: [],
    controllerRuns: [{ run_id: 'a', profile: 'gah', work_id: '#1380', started_at: '2026-10-04T11:54:43Z', finished_at: null, action: 'fix_existing: x', status: 'running', outcome: null }],
    ledgers: { '#1380': { effective_backend: 'claude', effective_model: 'sonnet', mode: 'fix' } as never },
    factoryAgents: [{ pid: 1, tool: 'codex', cwd: '/w/a', started_at: '2026-10-04T11:55:13Z', model: 'gpt-6-sol' }] });
  expect(retried.filter((row) => row.job).map((row) => [row.name, row.model, row.runId])).toEqual([['codex', 'gpt-6-sol', 'a']]);
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
  expect(agentDisplayName('agy:google-native')).toBe('Antigravity');
  expect(agentDisplayName('agy')).toBe('Antigravity');
  expect(agentDisplayName('codex')).toBe('Codex');
  expect(agentDisplayName('agyle')).toBe('Agyle');
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
  const live = page.getByRole('region', { name: 'Factory Agents Status' });
  const agents = live.getByRole('list', { name: 'Agents' }).getByRole('listitem');
  await expect(agents).toHaveCount(2);
  // The name line carries the model: "Work account Opus".
  await expect(agents.nth(0)).toContainText('Work account Opus');
  await expect(agents.nth(0)).toContainText(/improve on #946 for 5m \d\ds/);
  await expect(agents.nth(0)).toContainText('3 files changed · claimed 4m 50s ago · last attempt 1m');
  await expect(agents.nth(0).getByRole('img', { name: 'working' })).toBeVisible();
  await expect(agents.nth(1)).toContainText('Codex-work');
  await expect(agents.nth(1)).toContainText('disabled, routing skips it');
  await expect(live).toContainText('1 busy of 2');
  await expect(live.getByRole('heading', { name: 'Factory Agents Status' })).toHaveAttribute('title', 'Live: what each agent is doing now within the Factory');
  // The timer ticks while a row is busy.
  const before = await agents.nth(0).textContent();
  await expect.poll(async () => (await agents.nth(0).textContent()) !== before, { timeout: 5000 }).toBe(true);
});

test('Non Factory Agents lists device CLIs outside the factory and dashboard chats in use', async ({ page }) => {
  await page.route('**/api/device-agents', (route) => route.fulfill({ json: { supported: true, generated_at: new Date().toISOString(), factory_agents: [], agents: [
    { pid: 4242, tool: 'claude', model: 'opus-5-5', title: 'Fix the checkout flow', last_activity_at: new Date(Date.now() - 5_000).toISOString(), cwd: '/home/me/Projects/site', started_at: new Date(Date.now() - 12 * 60_000).toISOString() },
    { pid: 4250, tool: 'claude', model: null, title: 'Plan the launch', last_activity_at: new Date(Date.now() - 40 * 60_000).toISOString(), cwd: '/home/me/Projects/site', started_at: null },
    { pid: 4300, tool: 'codex', cwd: null, started_at: null }
  ] } }));
  await page.route('**/api/manager-chat/sessions/all', (route) => route.fulfill({ json: { projects: [{ profile: 'fixture', sessions: [
    { id: 'chat-1', profile: 'fixture', backend: 'codex', model: 'gpt-6-sol', title: 'Fix the retry loop', branch: 'gah/chat-1', outcome: 'live', lastActiveAt: Date.now() - 3 * 60_000, createdAt: Date.now() - 3_600_000, worktreePath: null, reasoningEffort: null },
    { id: 'chat-2', profile: 'fixture', backend: 'claude', model: null, title: 'Yesterday', branch: 'gah/chat-2', outcome: 'live', lastActiveAt: Date.now() - 26 * 3_600_000, createdAt: 0, worktreePath: null, reasoningEffort: null }
  ] }] } }));
  await page.goto('/?page=overview&profile=fixture');
  const panel = page.getByRole('region', { name: 'Non Factory Agents' });
  const rows = panel.getByRole('list', { name: 'Non factory agents' }).getByRole('listitem');
  await expect(rows).toHaveCount(4);
  await expect(panel).toContainText('4 open');
  // Working: a transcript written seconds ago. The title says what it is doing.
  await expect(rows.nth(0)).toContainText('Claude Opus-5-5');
  await expect(rows.nth(0)).toHaveAttribute('data-agent-state', 'working');
  await expect(rows.nth(0)).toContainText('Fix the checkout flow');
  await expect(rows.nth(0)).toContainText(/~\/Projects\/site · open 12m \d\ds · pid 4242/);
  // Idle: open, but nothing written for a while.
  await expect(rows.nth(1)).toHaveAttribute('data-agent-state', 'idle');
  await expect(rows.nth(1)).toContainText(/idle, waiting for 40m/);
  await expect(rows.nth(1)).toContainText('Plan the launch');
  // No transcript known: just running.
  await expect(rows.nth(2)).toHaveAttribute('data-agent-state', 'running');
  await expect(rows.nth(2).getByRole('img', { name: 'running' })).toHaveClass(/bg-good/);
  await expect(rows.nth(2)).toContainText('working directory not readable');
  await expect(rows.nth(3)).toContainText('Codex Gpt-6-sol');
  await expect(panel.getByText('Yesterday')).toHaveCount(0);
  await rows.nth(3).getByRole('button', { name: 'Fix the retry loop' }).click();
  await expect(page).toHaveURL(/[?&]chat=chat-1/);
});

test('Watch live opens a read-only view of a running job in the left sidebar and follows its output', async ({ page }) => {
  const RUN = '8c4013e4-2937-4bac-b258-115ae2b3d7e1';
  let polls = 0;
  await page.route('**/api/controller-activity**', (route) => route.fulfill({ json: [
    { run_id: RUN, profile: 'fixture', work_id: '#1381', started_at: new Date(Date.now() - 120_000).toISOString(), finished_at: null, action: 'dispatch_ticket: 1381', status: 'running', outcome: null }
  ] }));
  await page.route('**/api/device-agents', (route) => route.fulfill({ json: { supported: true, generated_at: new Date().toISOString(), agents: [],
    factory_agents: [{ pid: 1, tool: 'codex', model: 'gpt-6-sol', cwd: '/w/a', started_at: new Date(Date.now() - 110_000).toISOString() }] } }));
  await page.route(`**/api/factory-runs/${RUN}/output**`, (route) => {
    const after = Number(new URL(route.request().url()).searchParams.get('after'));
    polls++;
    if (after === 0) return route.fulfill({ json: { found: true, attempt: 1, next: 100, truncated: false, events: [
      { kind: 'message', text: 'I will trace the loop.' },
      { kind: 'command', text: 'cargo test --lib', output: null, status: 'running', exit_code: null }
    ] } });
    if (after === 100) return route.fulfill({ json: { found: true, attempt: 1, next: 200, truncated: false, events: [
      { kind: 'command', text: 'cargo test --lib', output: 'test result: FAILED', status: 'failed', exit_code: 101 },
      { kind: 'file_change', text: 'update src/controller/decision.rs', status: 'completed' }
    ] } });
    return route.fulfill({ json: { found: true, attempt: 1, next: 200, truncated: false, events: [] } });
  });
  await page.goto('/?page=overview&profile=fixture');
  const live = page.getByRole('region', { name: 'Factory Agents Status' });
  // The running job is named by the factory's agent process (its account and model), not "controller".
  await expect(live).toContainText('Mock-account Gpt-6-sol');
  await live.getByRole('button', { name: 'Watch #1381 live' }).click();
  const view = page.getByRole('complementary', { name: 'Live view' });
  await expect(view.getByRole('heading', { name: 'Mock-account gpt-6-sol on #1381' })).toBeVisible();
  await expect(view).toContainText('Live view · read only');
  const steps = view.getByRole('list', { name: 'Agent output' }).getByRole('listitem');
  await expect(steps.nth(0)).toContainText('I will trace the loop.');
  // The running command is replaced by its result, then the file change follows.
  await expect(steps).toHaveCount(3, { timeout: 8000 });
  await expect(steps.nth(1)).toContainText('failed (101)');
  await expect(steps.nth(1)).toContainText('test result: FAILED');
  await expect(steps.nth(2)).toContainText('update src/controller/decision.rs');
  await expect(view.getByRole('status')).toContainText('Following · 3 steps');
  // View only: nothing in it sends input or stops the job.
  await expect(view.getByRole('textbox')).toHaveCount(0);
  await expect(view.getByRole('button')).toHaveCount(1);
  expect(polls).toBeGreaterThan(1);
  await view.getByRole('button', { name: 'Close live view' }).click();
  await expect(page.getByRole('complementary', { name: 'Git issues' })).toBeVisible();
});
