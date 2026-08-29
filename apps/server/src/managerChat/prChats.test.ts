import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { listChatPrs, startPrChat } from './prChats.js';
import { startIssueChat } from './issueChats.js';
import { archiveSession, setChatSessionStoreOptions, listSessions } from './chatSessions.js';
import { setSessionLogOptions } from './ManagerChatManager.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../../tests/fixtures');

function initRepo(path: string): void {
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: path });
  execFileSync('git', ['config', 'user.email', 't@t'], { cwd: path });
  execFileSync('git', ['config', 'user.name', 't'], { cwd: path });
  writeFileSync(join(path, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: path });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: path });
}

interface TestEnv {
  root: string;
  profileInfo: ProfileSummary;
  stateFile: string;
  cleanup: () => void;
}

function withEnv(testFn: (env: TestEnv) => void | Promise<void>): () => Promise<void> {
  return async () => {
    const root = mkdtempSync(join(tmpdir(), 'gah-pr-chats-'));
    const checkout = join(root, 'checkout');
    execFileSync('mkdir', ['-p', checkout]);
    initRepo(checkout);
    const stateDir = join(root, 'state');
    const stateFile = join(root, 'gh-state.log');
    setChatSessionStoreOptions({ stateDir });
    setSessionLogOptions({ stateDir });
    const savedPath = process.env.PATH;
    process.env.GAH_FAKE_GH_FIXTURE = join(fixtures, 'gh/data');
    process.env.GAH_FAKE_GH_STATE = stateFile;
    process.env.PATH = `${join(fixtures, 'gh')}:${process.env.PATH}`;
    const env: TestEnv = {
      root,
      stateFile,
      profileInfo: {
        name: 'p',
        display_name: 'P',
        provider: 'github',
        repo: 'owner/repo',
        repo_id: 'repo',
        local_path: checkout,
        worktree_base: join(root, 'worktrees'),
        web_url: 'https://github.com/owner/repo',
        max_parallel_workers: null,
        max_open_managed_mrs: 1,
        manager_wake_autonomy: null,
        validation_timeout_seconds: 300
      },
      cleanup: () => {
        setChatSessionStoreOptions({ stateDir: undefined });
        setSessionLogOptions({ stateDir: undefined });
        process.env.PATH = savedPath;
        delete process.env.GAH_FAKE_GH_FIXTURE;
        delete process.env.GAH_FAKE_GH_STATE;
        execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
        rmSync(root, { recursive: true, force: true });
      }
    };
    try {
      await testFn(env);
    } finally {
      env.cleanup();
    }
  };
}

test('listChatPrs returns open PRs newest-first with author and draft/review state', withEnv(async (env) => {
  const prs = await listChatPrs(env.profileInfo);
  assert.deepEqual(prs.map((pr) => pr.number), [12, 11], 'merged PRs filtered, newest first');
  assert.equal(prs[0].title, 'Ship the PR chat mode');
  assert.equal(prs[0].author, 'octocat');
  assert.equal(prs[0].headRefName, 'feat/pr-chat');
  assert.equal(prs[0].isDraft, false);
  assert.equal(prs[0].reviewState, 'APPROVED');
  assert.equal(prs[1].isDraft, true, 'draft state reported');
  assert.equal(prs[1].reviewState, 'REVIEW_REQUIRED');
}));

test('startPrChat opens a read-only seeded session: no branch, no worktree, no PR mutation', withEnv(async (env) => {
  const { session, existing } = await startPrChat({
    profile: 'p',
    profileInfo: env.profileInfo,
    prNumber: 12,
    backend: 'hermes'
  });
  assert.equal(existing, false);
  assert.equal(session.branch, 'feat/pr-chat', 'session rides the PR head branch name');
  assert.equal(session.title, '#12 Ship the PR chat mode');
  assert.equal(session.worktreePath, null, 'no worktree materialized');
  assert.ok(!existsSync(env.profileInfo.worktree_base), 'worktree base never created');

  // Read-only: nothing was recorded at the provider (issue chats append an
  // in-progress edit; PR chats must never touch the PR).
  assert.ok(!existsSync(env.stateFile), 'no provider mutation');

  // The session log opens with the PR as the first message.
  const log = readFileSync(join(env.root, 'state', 'project-p', `session-${session.id}`, 'session.jsonl'), 'utf8');
  const events = log.trim().split('\n').map((line) => JSON.parse(line));
  assert.equal(events[0].type, 'turn/start');
  assert.equal(events[1].type, 'user/message');
  assert.match(events[1].text, /#12 Ship the PR chat mode/);
  assert.match(events[1].text, /Adds the PR tab to the new-chat modal/);
  assert.match(events[1].text, /Head branch: feat\/pr-chat/);
  assert.match(events[1].text, /\(https:\/\/github\.com\/owner\/repo\/pull\/12\)/);
  assert.equal(events[2].type, 'turn/end');

  // Idempotent open: a second start returns the SAME live session.
  const second = await startPrChat({ profile: 'p', profileInfo: env.profileInfo, prNumber: 12, backend: 'codex' });
  assert.equal(second.existing, true);
  assert.equal(second.session.id, session.id);
  assert.equal(listSessions('p').length, 1);

  // After archiving, a fresh open for the same PR starts a new session on
  // the same head branch (the name was never created, so nothing collides).
  await archiveSession('p', session.id, env.profileInfo);
  const third = await startPrChat({ profile: 'p', profileInfo: env.profileInfo, prNumber: 12, backend: 'hermes' });
  assert.equal(third.existing, false);
  assert.equal(third.session.branch, 'feat/pr-chat');
  assert.equal(third.session.worktreePath, null);
  assert.equal(listSessions('p').length, 2);
}));

test('startPrChat does not reuse a writable issue session on the PR head branch', withEnv(async (env) => {
  const issue = await startIssueChat({
    profile: 'p',
    profileInfo: env.profileInfo,
    issueNumber: 42,
    backend: 'codex',
    model: 'issue-model'
  });
  assert.equal(issue.session.branch, 'gah/issue/repo-42');
  assert.ok(issue.session.worktreePath, 'issue session has a writable worktree');
  const issueState = { ...issue.session };
  const providerState = readFileSync(env.stateFile, 'utf8');

  const started = await startPrChat({
    profile: 'p',
    profileInfo: env.profileInfo,
    prNumber: 13,
    backend: 'vibe',
    model: 'pr-model'
  });
  assert.equal(started.existing, false);
  assert.notEqual(started.session.id, issue.session.id);
  assert.equal(started.session.branch, issue.session.branch);
  assert.equal(started.session.worktreePath, null, 'PR chat is read-only and worktree-less');
  assert.equal(started.session.backend, 'vibe');
  assert.equal(started.session.model, 'pr-model');

  const reopened = await startPrChat({
    profile: 'p',
    profileInfo: env.profileInfo,
    prNumber: 13,
    backend: 'hermes',
    model: 'other-model'
  });
  assert.equal(reopened.existing, true);
  assert.equal(reopened.session.id, started.session.id);
  assert.equal(reopened.session.worktreePath, null);
  assert.deepEqual(listSessions('p').find((session) => session.id === issue.session.id), issueState);
  assert.equal(readFileSync(env.stateFile, 'utf8'), providerState, 'PR starts do not mutate issue provider state');
  assert.equal(listSessions('p').length, 2);
}));

test('startPrChat refuses closed and merged PRs loudly', withEnv(async (env) => {
  await assert.rejects(
    startPrChat({ profile: 'p', profileInfo: env.profileInfo, prNumber: 10, backend: 'hermes' }),
    /is MERGED, not open/
  );
  assert.equal(listSessions('p').length, 0, 'no session created for a merged PR');
  assert.ok(!existsSync(env.stateFile), 'nothing was mutated at the provider');
}));
