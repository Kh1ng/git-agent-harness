import test from 'node:test';
import assert from 'node:assert/strict';
import type { IssueIntakeRejection } from '@git-agent-harness/contracts';
import { ISSUE_MANAGED_ERROR_CODE, assertIssueNotManaged, issueWorkId, managedHold } from './managedIssues.js';

const rejection = (work_id: string | null, reason_code: string, reason = 'managed: assigned to colton, who is not this loop'): IssueIntakeRejection => ({
  ticket_path: work_id ?? 'x',
  work_id,
  title: 'Issue',
  provider: 'github',
  author_login: 'owner',
  author_kind: 'human',
  reason_code,
  reason,
  labels: []
});

const status = (...issue_intake_rejections: IssueIntakeRejection[]) => ({ issue_intake_rejections });

test('issueWorkId reads the issue forms a target can take', () => {
  assert.equal(issueWorkId('#1462'), '#1462');
  assert.equal(issueWorkId('1462'), '#1462');
  assert.equal(issueWorkId(' TICKET-0042 '), '#42');
  assert.equal(issueWorkId('docs/tickets/TICKET-42.md'), null);
  assert.equal(issueWorkId(undefined), null);
});

test('managedHold names only managed rejections of that issue', () => {
  const snapshot = status(rejection('#7', 'managed'), rejection('#8', 'untrusted_author'), rejection(null, 'managed'));
  assert.equal(managedHold(snapshot, '#7'), '#7 is managed (assigned to colton, who is not this loop); a manager owns it, so it cannot be started from here');
  assert.equal(managedHold(snapshot, '#8'), null);
  assert.equal(managedHold(snapshot, '#9'), null);
  assert.equal(managedHold({ issue_intake_rejections: [] }, '#7'), null);
});

test('an implementation dispatch of a managed issue is refused with ISSUE_MANAGED', async () => {
  const loads: string[] = [];
  const load = async (profile: string) => { loads.push(profile); return status(rejection('#7', 'managed')); };
  await assert.rejects(
    assertIssueNotManaged({ profile: 'gah', mode: 'fix', target: '#7' }, load),
    (error: unknown) => error instanceof Error && 'code' in error && error.code === ISSUE_MANAGED_ERROR_CODE && /#7 is managed/.test(error.message)
  );
  await assert.rejects(assertIssueNotManaged({ profile: 'gah', mode: 'improve', target: '7' }, load), /ISSUE|managed/);
  assert.deepEqual(loads, ['gah', 'gah']);
});

test('other issues, other modes and non-issue targets pass without a status read', async () => {
  let loads = 0;
  const load = async () => { loads += 1; return status(rejection('#7', 'managed')); };
  await assertIssueNotManaged({ profile: 'gah', mode: 'fix', target: '#8' }, load);
  assert.equal(loads, 1);
  await assertIssueNotManaged({ profile: 'gah', mode: 'review', target: '#7' }, load);
  await assertIssueNotManaged({ profile: 'gah', mode: 'pm', target: '#7' }, load);
  await assertIssueNotManaged({ profile: 'gah', mode: 'fix', target: 'docs/tickets/TICKET-7.md' }, load);
  await assertIssueNotManaged({ profile: 'gah', mode: 'fix' }, load);
  assert.equal(loads, 1, 'nothing else needed the snapshot');
});

test('an unreadable status snapshot does not block a manual start', async () => {
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message: string) => { warnings.push(message); };
  try {
    await assertIssueNotManaged({ profile: 'gah', mode: 'fix', target: '#7' }, async () => { throw new Error('gah status timed out'); });
  } finally {
    console.warn = original;
  }
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /without the managed check: gah status timed out/);
});

test('a slow status snapshot is given up on after the timeout', async () => {
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message: string) => { warnings.push(message); };
  const started = Date.now();
  try {
    await assertIssueNotManaged({ profile: 'gah', mode: 'fix', target: '#7' }, () => new Promise(() => {}), 20);
  } finally {
    console.warn = original;
  }
  assert.ok(Date.now() - started < 2_000);
  assert.match(warnings[0], /status took longer than 20 ms/);
});
