import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { ISSUE_IN_PROGRESS_LABEL, issueBranchName, listChatIssues, startIssueChat } from './issueChats.js';
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
    const root = mkdtempSync(join(tmpdir(), 'gah-issue-chats-'));
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

test('listChatIssues returns open issues newest-first', withEnv(async (env) => {
  const issues = await listChatIssues(env.profileInfo);
  assert.deepEqual(issues.map((issue) => issue.number), [42, 41], 'closed issues filtered, newest first');
  assert.equal(issues[0].title, 'Fix the retry loop');
  assert.deepEqual(issues[0].labels, ['bug']);
}));

test('startIssueChat branches, marks in progress, and seeds the log with the issue', withEnv(async (env) => {
  const { session, existing } = await startIssueChat({
    profile: 'p',
    profileInfo: env.profileInfo,
    issueNumber: 42,
    backend: 'hermes'
  });
  assert.equal(existing, false);
  assert.equal(session.branch, issueBranchName('repo', 42), 'branch named for the issue');
  assert.equal(session.title, '#42 Fix the retry loop');
  assert.ok(session.worktreePath && session.worktreePath.includes('gah-chat-repo-'), 'worktree dir keeps the prune-safe prefix');

  // The issue was marked in progress: @me assignee + the in-progress label.
  const edits = readFileSync(env.stateFile, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  assert.equal(edits.length, 1);
  assert.equal(Number(edits[0].number), 42);
  assert.ok(edits[0].args.includes('--add-assignee'));
  assert.ok(edits[0].args.includes('--add-label', ));
  assert.ok(edits[0].args.includes(ISSUE_IN_PROGRESS_LABEL));

  // The session log opens with the issue as the first message.
  const log = readFileSync(join(env.root, 'state', 'project-p', `session-${session.id}`, 'session.jsonl'), 'utf8');
  const events = log.trim().split('\n').map((line) => JSON.parse(line));
  assert.equal(events[0].type, 'turn/start');
  assert.equal(events[1].type, 'user/message');
  assert.match(events[1].text, /#42 Fix the retry loop/);
  assert.match(events[1].text, /retry loop spins forever/);
  assert.equal(events[2].type, 'turn/end');

  // The worktree actually sits on the issue branch.
  const branch = execFileSync('git', ['branch', '--show-current'], { cwd: session.worktreePath, encoding: 'utf8' }).trim();
  assert.equal(branch, session.branch);

  // Idempotent grab: a second start returns the SAME live session.
  const second = await startIssueChat({ profile: 'p', profileInfo: env.profileInfo, issueNumber: 42, backend: 'codex' });
  assert.equal(second.existing, true);
  assert.equal(second.session.id, session.id);
  assert.equal(readFileSync(env.stateFile, 'utf8').trim().split('\n').length, 1, 'no duplicate in-progress edit');

  // After archiving, a fresh grab gets a suffixed branch (branches survive
  // archive by design, so the canonical name is taken).
  await archiveSession('p', session.id, env.profileInfo);
  const third = await startIssueChat({ profile: 'p', profileInfo: env.profileInfo, issueNumber: 42, backend: 'hermes' });
  assert.equal(third.existing, false);
  assert.match(third.session.branch, /^gah\/issue\/repo-42-[0-9a-z]+$/, 'suffixed branch after archive');
  assert.equal(listSessions('p').length, 2);
}));

test('startIssueChat refuses closed issues loudly', withEnv(async (env) => {
  // The fixture's issue 40 reports state closed.
  writeFileSync(join(fixtures, 'gh/data/issue-40.json'), JSON.stringify({
    number: 40, title: 'Closed thing', body: '', state: 'closed',
    url: null, labels: []
  }));
  await assert.rejects(
    startIssueChat({ profile: 'p', profileInfo: env.profileInfo, issueNumber: 40, backend: 'hermes' }),
    /is closed, not open/
  );
  assert.ok(!existsSync(env.stateFile), 'nothing was marked in progress');
}));
