import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { ChatSessionSummary, MergeRequest, ProfileSummary } from '@git-agent-harness/contracts';
import type { SettleDetails } from './chatSessions.js';
import {
  issueNumberForBranch,
  reclaimChatSessions,
  runChatMaintenanceTick,
  selectReclaimCandidates,
  type ChatMaintenanceDeps
} from './chatMaintenance.js';

const NOW = 1_800_000_000_000;

function profile(): ProfileSummary {
  return {
    name: 'repo',
    display_name: 'Repo',
    provider: 'github',
    repo: 'owner/repo',
    local_path: '/tmp/repo',
    repo_id: 'my-repo',
    worktree_base: '/tmp/worktrees',
    web_url: null,
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    delivery_mode: 'pr',
    validation_timeout_seconds: 300,
    chat_session_idle_days: 14
  };
}

function session(id: string, branch: string, ageDays: number, outcome: ChatSessionSummary['outcome'] = 'live'): ChatSessionSummary {
  return {
    id,
    profile: 'repo',
    worktreePath: `/missing/${id}`,
    branch,
    backend: 'codex',
    model: null,
    title: id,
    createdAt: NOW - ageDays * 86_400_000,
    lastActiveAt: NOW - ageDays * 86_400_000,
    archivedAt: outcome === 'live' ? null : NOW,
    outcome,
    settledAt: outcome === 'settled' ? NOW : null,
    settledReason: outcome === 'settled' ? 'merged' : null
  };
}

function mergeRequest(branch: string, classification: string): MergeRequest {
  return {
    branch,
    id: '1',
    url: null,
    state: classification === 'MERGED' ? 'closed' : 'open',
    draft: false,
    merge_status: null,
    merged: classification === 'MERGED',
    ci_passed: true,
    ci_pending: false,
    review_contract_version: 1,
    classification,
    recommended_action: 'NONE'
  };
}

test('candidate selection prioritizes terminal provider state, then idle age, and skips active/closed sessions', () => {
  const sessions = [
    session('merged', 'gah/chat/my-repo-merged', 1),
    session('issue', 'gah/issue/my-repo-990', 1),
    session('idle', 'gah/chat/my-repo-idle', 20),
    session('active', 'gah/chat/my-repo-active', 20),
    session('recent', 'gah/chat/my-repo-recent', 2),
    session('already', 'gah/chat/my-repo-already', 30, 'archived')
  ];
  const candidates = selectReclaimCandidates({
    profile: profile(),
    sessions,
    mergeRequests: [mergeRequest(sessions[0].branch, 'MERGED')],
    closedIssues: new Set([990]),
    activeSessionIds: new Set(['active']),
    now: NOW
  });
  assert.deepEqual(candidates.map(({ sessionId, outcome, reason }) => ({ sessionId, outcome, reason })), [
    { sessionId: 'merged', outcome: 'settled', reason: 'merged' },
    { sessionId: 'issue', outcome: 'settled', reason: 'closed' },
    { sessionId: 'idle', outcome: 'archived', reason: 'idle' }
  ]);
});

test('issue branch parsing is repo-scoped and accepts post-archive suffixes', () => {
  assert.equal(issueNumberForBranch('gah/issue/my-repo-990', 'my-repo'), 990);
  assert.equal(issueNumberForBranch('gah/issue/my-repo-990-mfrgg', 'my-repo'), 990);
  assert.equal(issueNumberForBranch('gah/issue/other-990', 'my-repo'), null);
});

test('dry run is non-mutating and reclaim uses the shared archive callback with settlement reason', async () => {
  const sessions = [session('merged', 'gah/chat/my-repo-merged', 1), session('idle', 'gah/chat/my-repo-idle', 20)];
  const calls: { id: string; reason?: string }[] = [];
  const deps: ChatMaintenanceDeps = {
    listProfiles: async () => [profile()],
    sync: async () => [mergeRequest(sessions[0].branch, 'MERGED')],
    issueState: async () => 'open',
    listSessions: () => sessions,
    archive: async (_profile, id, settlement) => {
      calls.push({ id, reason: settlement?.reason });
      return { ...sessions.find((candidate) => candidate.id === id)!, outcome: settlement ? 'settled' : 'archived' };
    },
    isActive: () => false,
    now: () => NOW
  };

  const dryRun = await reclaimChatSessions({ profile: 'repo', dryRun: true }, deps);
  assert.equal(dryRun.candidates.length, 2);
  assert.deepEqual(calls, []);

  const reclaimed = await reclaimChatSessions({ profile: 'repo', dryRun: false }, deps);
  assert.equal(reclaimed.sessions.length, 2);
  assert.deepEqual(calls, [{ id: 'merged', reason: 'merged' }, { id: 'idle', reason: undefined }]);
});

test('the sweep threads provider details so the settled event records the PR or issue (#1036)', async () => {
  const sessions = [
    session('merged', 'gah/chat/my-repo-merged', 1),
    session('issue', 'gah/issue/my-repo-990', 1),
    session('idle', 'gah/chat/my-repo-idle', 20)
  ];
  const details: { id: string; details?: SettleDetails }[] = [];
  const deps: ChatMaintenanceDeps = {
    listProfiles: async () => [profile()],
    sync: async () => [mergeRequest(sessions[0].branch, 'MERGED')],
    issueState: async () => 'closed',
    listSessions: () => sessions,
    archive: async (_profile, id, settlement, settleDetails) => {
      details.push({ id, details: settleDetails });
      return { ...sessions.find((candidate) => candidate.id === id)!, outcome: settlement ? 'settled' : 'archived' };
    },
    isActive: () => false,
    now: () => NOW
  };

  await reclaimChatSessions({ profile: 'repo', dryRun: false }, deps);
  assert.deepEqual(details, [
    { id: 'merged', details: { pullRequest: { id: '1', url: null, sourceSha: null } } },
    { id: 'issue', details: { issue: { number: 990 } } },
    { id: 'idle', details: undefined }
  ]);
});

test('the maintenance tick sweeps only profiles with live sessions', async () => {
  const live = profile();
  const quiet: ProfileSummary = { ...profile(), name: 'quiet' };
  const swept: (string | undefined)[] = [];
  const deps: ChatMaintenanceDeps = {
    listProfiles: async () => [live, quiet],
    sync: async () => [],
    issueState: async () => 'open',
    listSessions: (name) => name === 'repo' ? [session('live-one', 'gah/chat/my-repo-live', 0)] : [],
    archive: async (name, id) => {
      swept.push(name);
      return { ...session(id, 'gah/chat/my-repo-live', 0), outcome: 'settled', settledReason: 'delivered', settledAt: NOW };
    },
    isActive: () => false,
    now: () => NOW
  };

  const sweptProfiles = await runChatMaintenanceTick(deps);
  assert.deepEqual(sweptProfiles, ['repo'], 'the profile without live sessions is not swept');
});
