import type { AvailableTicket, IssueIntakeRejection, StatusSnapshot } from '@git-agent-harness/contracts';
import { buildKanban, managedIssues } from './kanbanBoard.js';

const rejection = (work_id: string | null, reason_code: string, reason: string): IssueIntakeRejection => ({
  ticket_path: work_id ?? 'issue', work_id, title: `Issue ${work_id}`, provider: 'github', author_login: 'owner', author_kind: 'human', reason_code, reason, labels: []
});
const ticket = (work_id: string, human_required_reason_code: string | null = null): AvailableTicket =>
  ({ work_id, ticket_path: work_id, title: `Ticket ${work_id}`, prior_attempt_count: 0, human_required_reason_code }) as unknown as AvailableTicket;
const status = (overrides: Partial<StatusSnapshot>): StatusSnapshot =>
  ({ profile: { max_fix_attempts_per_mr: 2, max_open_managed_mrs: 4 }, max_parallel_workers: 2, open_managed_mr_count: 0, implementation_intake_paused: false, blockers: [], available_tickets: [], merge_requests: [], active_claims: [], blocked_work_items: [], dependency_blockers: [], review_held_work_ids: [], running_workers: [], issue_intake_rejections: [], ...overrides }) as unknown as StatusSnapshot;
const board = (snapshot: StatusSnapshot) => buildKanban({ status: snapshot, quota: null, controllerRuns: [], factoryAgents: [], loopRunning: true, now: Date.now() });

test('managedIssues keys only the managed rejections by work key', () => {
  const managed = managedIssues(status({ issue_intake_rejections: [
    rejection('#7', 'managed', 'managed: assigned to colton, who is not this loop'),
    rejection('#8', 'untrusted_author', 'author is not trusted by this profile'),
    rejection(null, 'managed', 'managed: the managed label is on it')
  ] }));
  expect([...managed]).toEqual([['#7', 'Managed: assigned to colton, who is not this loop']]);
  expect(managedIssues(null).size).toBe(0);
});

test('a managed issue is marked on its card, turns Assign off and is flagged among the not-picked-up', () => {
  const result = board(status({
    available_tickets: [ticket('#7', 'stuck_loop_gate'), ticket('#9')],
    issue_intake_rejections: [rejection('TICKET-0007', 'managed', 'managed: the managed label is on it, so a manager owns this issue'), rejection('#8', 'blocked', 'blocked label present')]
  }));
  const seven = result.cards.find((card) => card.workId === '#7');
  const nine = result.cards.find((card) => card.workId === '#9');
  expect(seven?.managed).toBe('Managed: the managed label is on it, so a manager owns this issue');
  expect(seven?.assignHeldBy).toMatch(/^Assign is off: a manager owns this issue/);
  expect(seven?.blocks).toContain('Managed: the managed label is on it, so a manager owns this issue');
  expect(nine?.managed).toBeNull();
  expect(nine?.assignHeldBy).toBeNull();
  expect(result.notPickedUp.map((item) => [item.workId, item.managed])).toEqual([['TICKET-0007', true], ['#8', false]]);
});
