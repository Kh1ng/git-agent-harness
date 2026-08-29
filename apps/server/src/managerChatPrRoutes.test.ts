import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ChatPrStartResult, ChatPrSummary, ChatSessionSummary } from '@git-agent-harness/contracts';
import { createServer } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

/** The PR routes through the real express app against the fixture gah +
 * fake gh: list returns normalized open PRs, start opens a read-only
 * seeded session without creating a worktree or touching the PR. */
test('manager-chat PR routes list and start through the fixture provider', { timeout: 30_000 }, async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-pr-routes-'));
  const checkout = join(root, 'checkout');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 't@t'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 't'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'pr-routes';
  const profileListPath = join(root, 'profile-list.json');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'PR Routes',
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
  }]));

  const savedEnv = { ...process.env };
  process.env.GAH_BINARY = join(fixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.PATH = `${join(fixtures, 'gh')}:${process.env.PATH}`;
  process.env.GAH_FAKE_GH_FIXTURE = join(fixtures, 'gh/data');
  process.env.GAH_FAKE_GH_STATE = join(root, 'gh-state.log');
  process.env.GAH_CHAT_STATE_DIR = join(root, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(root, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(root, 'manager-chat.json');
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'coordinator-identity.json');
  resetCachedCoordinatorIdentity();

  const app = createServer({});
  const server = http.createServer(app);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  try {
    const listResponse = await fetch(`${baseUrl}/api/manager-chat/prs?profile=${profile}`);
    assert.equal(listResponse.status, 200);
    const { prs } = await listResponse.json() as { prs: ChatPrSummary[] };
    assert.deepEqual(prs.map((pr) => pr.number), [12, 11], 'merged PRs filtered, newest first');
    assert.equal(prs[0].author, 'octocat');
    assert.equal(prs[0].reviewState, 'APPROVED');
    assert.equal(prs[1].isDraft, true);

    // Missing prNumber is a 400, mirroring the issues/start route.
    const missing = await fetch(`${baseUrl}/api/manager-chat/prs/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile })
    });
    assert.equal(missing.status, 400);

    const startResponse = await fetch(`${baseUrl}/api/manager-chat/prs/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile, prNumber: 12, backend: 'codex' })
    });
    assert.equal(startResponse.status, 201);
    const { session, existing } = await startResponse.json() as ChatPrStartResult;
    assert.equal(existing, false);
    assert.equal(session.branch, 'feat/pr-chat', 'session rides the PR head branch name');
    assert.equal(session.worktreePath, null, 'no worktree created');
    assert.equal(session.title, '#12 Ship the PR chat mode');
    assert.ok(!existsSync(join(root, 'worktrees')), 'worktree base never created');
    assert.ok(!existsSync(process.env.GAH_FAKE_GH_STATE!), 'no provider mutation');

    // The session is listed and its log opens with the PR as the first message.
    const sessionsResponse = await fetch(`${baseUrl}/api/manager-chat/sessions?profile=${profile}`);
    assert.equal(sessionsResponse.status, 200);
    const { sessions } = await sessionsResponse.json() as { sessions: ChatSessionSummary[] };
    assert.ok(sessions.some((entry) => entry.id === session.id));
    const logText = readFileSync(
      join(root, 'chat', `project-${profile}`, `session-${session.id}`, 'session.jsonl'), 'utf8'
    );
    assert.match(logText, /#12 Ship the PR chat mode/);
    assert.match(logText, /Head branch: feat\/pr-chat/);

    // Idempotent open: a second start returns the same live session.
    const secondResponse = await fetch(`${baseUrl}/api/manager-chat/prs/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile, prNumber: 12, backend: 'codex' })
    });
    assert.equal(secondResponse.status, 201);
    const second = await secondResponse.json() as ChatPrStartResult;
    assert.equal(second.existing, true);
    assert.equal(second.session.id, session.id);

    // A merged PR is refused loudly, surfaced as the route's 502 shape.
    const mergedResponse = await fetch(`${baseUrl}/api/manager-chat/prs/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile, prNumber: 10, backend: 'codex' })
    });
    assert.equal(mergedResponse.status, 502);
    const merged = await mergedResponse.json() as { error: string; message: string };
    assert.equal(merged.error, 'Failed to start chat from pull request');
    assert.match(merged.message, /is MERGED, not open/);
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
    process.env = savedEnv;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  }
});
