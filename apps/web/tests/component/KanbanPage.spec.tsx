import { expect, test } from '@playwright/experimental-ct-react';
import type { AvailableTicket, ControllerActivity, DeviceAgent, MergeRequest, QuotaCandidateStatus, StatusSnapshot } from '@git-agent-harness/contracts';
import { buildKanban, parseSkipped, runSkipped, workingNow, whyNotRunning, type KanbanInput } from '../../src/lib/kanbanBoard.js';
import { WorkingNowMenu } from '../../src/components/WorkingNowMenu.js';
import { KanbanView } from '../../src/pages/KanbanPage.js';

const NOW = Date.parse('2026-10-05T18:30:00Z');

const ticket = (work_id: string, fields: Partial<AvailableTicket> = {}) => ({
  ticket_path: work_id.slice(1),
  work_id,
  normalized_work_identity: work_id,
  title: `Issue ${work_id}`,
  prior_attempt_count: 0,
  has_active_mr: false,
  has_active_claim: false,
  human_required: false,
  last_failure_class: null,
  execution_policy: { dispatchable_now: true, exclusion_reason: null, exclusion_reason_code: null },
  ...fields
}) as AvailableTicket;

const mergeRequest = (work_id: string, classification: string, fields: Partial<MergeRequest> = {}) => ({
  work_id,
  branch: `gah/job-${work_id.slice(1)}`,
  id: `9${work_id.slice(1)}`,
  url: `https://example.test/pull/9${work_id.slice(1)}`,
  draft: false,
  merged: false,
  ci_passed: true,
  ci_pending: false,
  classification,
  ...fields
}) as MergeRequest;

const candidate = (backend: string, model: string, modes: string[], fields: Partial<QuotaCandidateStatus> = {}) => ({
  backend, backend_instance: backend, model, modes, configured: true, eligible_now: true, quota_observations: [], ...fields
}) as QuotaCandidateStatus;

const status = {
  profile: { max_fix_attempts_per_mr: 2, max_open_managed_mrs: 15 },
  available_tickets: [
    ticket('#1'),
    ticket('#2', { prior_attempt_count: 3, last_failure_class: 'harness_error' }),
    ticket('#3', { has_active_mr: true }),
    ticket('#4', { has_active_mr: true, prior_attempt_count: 2 }),
    ticket('#5', { has_active_mr: true, human_required: true, human_required_reason_code: 'stuck_loop_gate' })
  ],
  merge_requests: [
    mergeRequest('#3', 'NEEDS_REVIEW', { effective_backend: 'agy', effective_model: 'Gemini 3.1 Pro (High)' }),
    mergeRequest('#4', 'NEEDS_FIX'),
    mergeRequest('#5', 'READY_FOR_HUMAN'),
    mergeRequest('#6', 'MERGED', { merged: true, merged_at: '2026-10-05T17:30:00Z', title: 'Issue #6' })
  ],
  active_claims: [{ work_id: '#4', pid: 1, scope: 'gah', hostname: 'host', claimed_at: '2026-10-05T18:13:00Z', age_seconds: 1020 }],
  running_workers: [{ work_id: '#4', run_id: 'fix', mode: 'fix', backend: 'claude', runner: 'claude', backend_instance: 'claude', model: 'opus[1m]', requested_model: 'opus[1m]', actual_model: null, node_id: 'node', branch: 'gah/job-4', started_at: '2026-10-05T18:13:00Z', last_activity_at: '2026-10-05T18:30:00Z', attempt: 1, stale_after_seconds: 900, state: 'running' }],
  blocked_work_items: [{ kind: 'human_required', reason_code: 'stuck_loop_gate', source_reference: '#5', message: "stuck-loop detected: 'merge_mr' selected 3 times in a row" }],
  blockers: [],
  constraints: [],
  issue_intake_rejections: [{ ticket_path: '7', work_id: '#7', title: 'Unlabelled idea', reason: "missing canonical autonomous label 'agents:me'" }],
  fix_attempt_counts: { 'gah/job-4': 1 },
  review_held_work_ids: [],
  max_parallel_workers: 4,
  open_managed_mr_count: 3,
  implementation_intake_paused: false
} as unknown as StatusSnapshot;

const controllerRuns: ControllerActivity[] = [
  { run_id: 'fix', profile: 'gah', work_id: '#4', started_at: '2026-10-05T18:13:00Z', finished_at: null, action: 'fix_existing: gah/job-4', status: 'running', outcome: null },
  {
    run_id: 'review', profile: 'gah', work_id: '#3', started_at: '2026-10-05T18:20:00Z', finished_at: '2026-10-05T18:20:03Z', action: 'review: gah/job-3', status: 'failed',
    outcome: 'review: deferred_capacity: no eligible backend available for preferred claude/opus[1m]; skipped: claude/opus[1m]: max_concurrent_reached, codex/gpt-6.1-sol: model-specific authentication_error route_state=58680e12'
  },
  { run_id: 'retry', profile: 'gah', work_id: '#2', started_at: '2026-10-05T18:10:00Z', finished_at: '2026-10-05T18:10:01Z', action: 'retry: 2', status: 'failed', outcome: 'retry: refusing to replace existing checkpoint worktree path /tmp/wip' }
];

const factoryAgents: DeviceAgent[] = [{ pid: 2, tool: 'claude', cwd: '/home/me/worktrees/gah-job-4', started_at: '2026-10-05T18:13:30Z', model: 'opus[1m]' }];

const input: KanbanInput = {
  status,
  quota: { candidates: [
    candidate('claude', 'sonnet', ['improve', 'review']),
    candidate('codex', 'gpt-6.1-sol', ['improve', 'review'], { eligible_now: false, reason: 'authentication_error' }),
    candidate('agy', 'Gemini 3.1 Pro (High)', ['improve'])
  ] },
  controllerRuns,
  factoryAgents,
  loopRunning: true,
  now: NOW
};
const board = buildKanban(input);

test('reads each skipped route out of a deferred run', () => {
  expect(parseSkipped(controllerRuns[1].outcome)).toEqual([
    { backend: 'claude', model: 'opus[1m]', reason: 'max_concurrent_reached' },
    { backend: 'codex', model: 'gpt-6.1-sol', reason: 'model-specific authentication_error' }
  ]);
  expect(parseSkipped('agy:google-native/Gemini 3.1 Pro (High): max_concurrent_reached')).toEqual([]);
});

test('the list on the run record is used as recorded, whatever the sentence says', () => {
  const run = {
    outcome: 'review: the router rewrote this sentence and names nobody',
    skipped: [{ backend: 'agy', backend_instance: 'agy:google-native', model: 'Gemini 3.1 Pro (High)', reason: 'max_concurrent_reached', unavailable_until: null }]
  };
  expect(runSkipped(run)).toEqual([{ backend: 'agy', backendInstance: 'agy:google-native', model: 'Gemini 3.1 Pro (High)', reason: 'max_concurrent_reached' }]);
  expect(runSkipped({ outcome: controllerRuns[1].outcome, skipped: [] })).toEqual([]);
  // A run recorded before the list existed still reads the sentence.
  expect(runSkipped({ outcome: controllerRuns[1].outcome })).toHaveLength(2);

  const reworded = controllerRuns.map((item) => (item.run_id === 'review' ? { ...item, ...run } : item));
  const review = buildKanban({ ...input, controllerRuns: reworded }).cards.find((card) => card.workId === '#3');
  expect(review?.reason).toBe('Waiting for a reviewer: Antigravity at its job limit');
});

test('every job sits in the column its records put it in, with a reason in plain words', async ({ mount }) => {
  const component = await mount(<KanbanView board={board} now={NOW} onOpenWork={() => {}} />);
  const column = (name: string) => component.getByRole('region', { name: new RegExp(`^${name}, `) });

  await expect(column('Ready')).toContainText("Next in line: starts on the loop's next pass");
  await expect(column('Waiting')).toContainText('Retry refused: saved work from an earlier attempt is still in the way');
  await expect(column('Waiting')).toContainText('attempt 3');
  await expect(column('In review')).toContainText('Waiting for a reviewer: Claude at its job limit, Codex sign-in failed');
  await expect(column('In review')).toContainText('built by Antigravity · Gemini 3.1 Pro (High)');
  await expect(column('Fixing')).toContainText('An agent is fixing what review or CI flagged');
  await expect(column('Fixing')).toContainText('Claude · opus[1m] · attempt 2 · fix 1 of 2 · working 17m');
  await expect(column('Needs you')).toContainText('Stuck: the same step repeated with no change, so it was stopped');
  await expect(column('Done')).toContainText('merged 1h ago');
  await expect(column('Running')).toContainText('Nothing here');
  await expect(component.getByText('Not picked up (1)')).toBeVisible();
});

test('the navbar names the job in hand; hovering lists each agent with its exact model and why it is idle', async ({ mount }) => {
  const { jobs, agents } = workingNow(input);
  const component = await mount(<WorkingNowMenu jobs={jobs} agents={agents} onOpenBoard={() => {}} />);
  const chip = component.getByRole('button', { name: /^Working on #4/ });
  await expect(chip).toHaveText('Working on #4');
  await chip.hover();

  const menu = component.getByRole('dialog', { name: 'What the factory is doing now' });
  // The running process reports the exact model; the route only said `sonnet`.
  await expect(menu).toContainText('#4 (fix)');
  await expect(menu).toContainText('Claude · opus[1m] · ');
  const rows = menu.getByRole('listitem');
  await expect(rows.nth(1)).toContainText('Working on #4 (fix)');
  await expect(rows.nth(2)).toContainText('Antigravity · Gemini 3.1 Pro (High)');
  await expect(rows.nth(2)).toContainText("Free: picks up work on the loop's next pass");
  await expect(rows.nth(3)).toContainText('Codex · gpt-6.1-sol');
  await expect(rows.nth(3)).toContainText('Sign-in failed; it needs to be logged in again');
});

test('before the status snapshot loads, the navbar does not infer workers from controller events', () => {
  const { jobs, agents } = workingNow({ ...input, status: null });
  expect(jobs).toEqual([]);
  expect(agents.find((agent) => agent.backend === 'agy')?.reason).toBe('Idle');
});

test('navbar jobs and capacity use the roster despite conflicting claims and controller events', () => {
  const snapshot = { ...status, running_workers: [] };
  expect(workingNow({ ...input, status: snapshot }).jobs).toEqual([]);
  expect(buildKanban({ ...input, status: snapshot }).gates.some(gate => gate.detail === '0 of 4 busy')).toBe(true);
});

test('the board has no agents panel: nothing sits beside it until a card is selected', async ({ mount }) => {
  const component = await mount(<KanbanView board={board} now={NOW} onOpenWork={() => {}} />);
  await expect(component.getByRole('region', { name: 'Agents' })).toHaveCount(0);
  await expect(component.getByRole('complementary')).toHaveCount(0);
});

test('a waiting card answers why it is not running, agent by agent', async ({ mount }) => {
  const component = await mount(<KanbanView board={board} now={NOW} onOpenWork={() => {}} />);
  await component.getByRole('button', { name: /Issue #3/ }).click();

  const why = component.getByRole('region', { name: "Why isn't #3 running?" });
  await expect(why).toContainText('Worker slots: 1 of 4 busy');
  await expect(why).toContainText('Claude · opus[1m]: Already running as many jobs as it is allowed at once (opus[1m])');
  await expect(why).toContainText('Antigravity · Gemini 3.1 Pro (High): Not set up for review jobs');
  await expect(why).toContainText('Codex · gpt-6.1-sol: Sign-in failed; it needs to be logged in again (gpt-6.1-sol)');

  await why.getByRole('button', { name: 'Close' }).click();
  await expect(why).toHaveCount(0);
});

// #5 without the stuck-loop stop, under an explicitly manual merge policy.
const approvedBoard = buildKanban({
  ...input,
  status: {
    ...status,
    profile: { ...status.profile, merge_policy: 'stop_for_human' },
    available_tickets: status.available_tickets.map((item) => (item.work_id === '#5' ? ticket('#5', { has_active_mr: true }) : item)),
    blocked_work_items: []
  } as StatusSnapshot
});

test('a skipped subscription does not block another account using the same backend', () => {
  const scoped = buildKanban({
    ...input,
    factoryAgents: [],
    quota: { candidates: [
      candidate('claude', 'opus', ['review'], { backend_instance: 'claude-first' }),
      candidate('claude', 'opus', ['review'], { backend_instance: 'claude-second' })
    ] },
    controllerRuns: [{ ...controllerRuns[1], skipped: [{ backend: 'claude', backend_instance: 'claude-first', model: 'opus', reason: 'authentication_error' }] }]
  });
  const verdicts = whyNotRunning(scoped, scoped.cards.find((card) => card.workId === '#3')!);
  expect(verdicts.find(({ agent }) => agent.id === 'claude-first')?.verdict).toContain('Sign-in failed');
  expect(verdicts.find(({ agent }) => agent.id === 'claude-second')).toMatchObject({ ok: true, verdict: 'Available' });
  expect(scoped.agents.find((agent) => agent.id === 'claude-second')?.reason).not.toContain('passed over');
});

test('approved PRs follow controller lifecycle gates rather than always needing a human', async ({ mount }) => {
  const approved = (fields: Partial<MergeRequest>, snapshot: Partial<StatusSnapshot> = {}) => buildKanban({
    ...input,
    controllerRuns: [],
    factoryAgents: [],
    status: {
      ...status,
      profile: { ...status.profile, merge_policy: 'squash' },
      available_tickets: [], active_claims: [], blocked_work_items: [],
      publishing_allow_pr: true,
      merge_requests: [mergeRequest('#8', 'READY_FOR_HUMAN', fields)],
      ...snapshot
    }
  });
  const automatic = approved({});
  expect(automatic.cards[0]).toMatchObject({ column: 'review', job: 'merge', reason: 'Approved and CI passed: waiting for the controller to merge' });
  expect(approved({ ci_passed: false, ci_pending: true }).cards[0]).toMatchObject({ column: 'waiting', reason: 'Approved: waiting for CI to finish before merging' });
  expect(approved({}, { review_held_work_ids: ['#8'] }).cards[0]).toMatchObject({ column: 'waiting', held: true });
  expect(approved({ draft: true }).cards[0]).toMatchObject({ column: 'review', reason: 'Approved: waiting for the controller to mark the draft ready' });
  expect(approved({}, { publishing_allow_pr: false }).cards[0]).toMatchObject({ column: 'needs_you' });
  expect(approvedBoard.cards.find((card) => card.workId === '#5')).toMatchObject({ column: 'needs_you' });

  const component = await mount(<KanbanView board={automatic} now={NOW} onOpenWork={() => {}} assign={{ unavailable: null, send: () => {} }} />);
  await component.getByRole('button', { name: /PR 98/ }).click();
  await expect(component.getByText('The controller handles this step; it does not need a coding agent.')).toBeVisible();
  await expect(component.getByRole('button', { name: 'Assign' })).toHaveCount(0);
});

test('Assign is off, with the reason shown, on a job held by a gate a manual fix does not lift', async ({ mount }) => {
  const sent: string[] = [];
  const component = await mount(
    <KanbanView board={board} now={NOW} onOpenWork={() => {}} assign={{ unavailable: null, send: (card) => sent.push(String(card.workId)) }} />
  );
  const needsYou = component.getByRole('region', { name: /^Needs you, / });
  await expect(needsYou.getByRole('button', { name: 'Assign' })).toBeDisabled();
  await expect(needsYou).toContainText('Assign is off: this job was stopped for repeating the same step');
  expect(sent).toEqual([]);
});

test('a Needs you job can be handed to an agent that is available', async ({ mount }) => {
  const sent: string[] = [];
  const component = await mount(
    <KanbanView board={approvedBoard} now={NOW} onOpenWork={() => {}} assign={{ unavailable: null, send: (card, agent) => sent.push(`${card.workId} -> ${agent.backend}`) }} />
  );
  const needsYou = component.getByRole('region', { name: /^Needs you, / });
  const picker = needsYou.getByRole('combobox', { name: 'Agent for #5' });

  // Codex cannot sign in, so it cannot be chosen.
  await expect(picker.getByRole('option', { name: 'Codex (unavailable)' })).toBeDisabled();
  await picker.selectOption({ label: 'Antigravity' });
  await needsYou.getByRole('button', { name: 'Assign' }).click();

  await expect(needsYou.getByRole('status')).toContainText('Sent to Antigravity');
  expect(sent).toEqual(['#5 -> agy']);
  // Only Needs you cards offer it.
  await expect(component.getByRole('button', { name: 'Assign' })).toHaveCount(1);
});
