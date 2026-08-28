import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import {
  archiveSession,
  chatKey,
  createSession,
  getSession,
  listSessions,
  resolveSessionCwd,
  setChatSessionStoreOptions,
  touchSession,
  updateSession
} from './chatSessions.js';

function initRepo(path: string): void {
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: path });
  execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: path });
  execFileSync('git', ['config', 'user.name', 'gah test'], { cwd: path });
  writeFileSync(join(path, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: path });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: path });
}

interface TestEnv {
  stateDir: string;
  checkout: string;
  worktreeBase: string;
  profileInfo: Pick<ProfileSummary, 'repo_id' | 'local_path' | 'worktree_base'>;
}

function withEnv(testFn: (env: TestEnv) => void | Promise<void>): () => Promise<void> {
  return async () => {
    const root = mkdtempSync(join(tmpdir(), 'gah-chat-sessions-'));
    const checkout = join(root, 'checkout');
    const worktreeBase = join(root, 'worktrees');
    const stateDir = join(root, 'state');
    execFileSync('mkdir', ['-p', checkout]);
    initRepo(checkout);
    setChatSessionStoreOptions({ stateDir });
    try {
      await testFn({ stateDir, checkout, worktreeBase, profileInfo: { repo_id: 'repo', local_path: checkout, worktree_base: worktreeBase } });
    } finally {
      setChatSessionStoreOptions({ stateDir: undefined });
      execFileSync('git', ['worktree', 'prune'], { cwd: checkout });
      rmSync(root, { recursive: true, force: true });
    }
  };
}

test('createSession makes a worktree and branch under the prune-recognized names', withEnv(async (env) => {
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  assert.match(session.branch, /^gah\/chat\/repo-[0-9a-f]{8}$/);
  assert.ok(session.worktreePath && session.worktreePath.startsWith(env.worktreeBase));
  assert.ok(existsSync(session.worktreePath!), 'worktree materialized');
  const dirs = readdirSync(env.worktreeBase);
  assert.ok(dirs.some((d) => d.startsWith('gah-chat-repo-')), `worktree dir uses the gah-chat prefix: ${dirs.join()}`);
  const branches = execFileSync('git', ['branch', '--list', session.branch], { cwd: env.checkout, encoding: 'utf8' });
  assert.ok(branches.includes(session.branch), 'branch exists in the main checkout');
  assert.equal(listSessions('p')[0].id, session.id);
}));

test('createSession without a worktree_base still works (checkout mode)', withEnv(async (env) => {
  const session = await createSession({
    profile: 'p',
    profileInfo: { repo_id: 'repo', local_path: env.checkout, worktree_base: '' },
    backend: 'codex'
  });
  assert.equal(session.worktreePath, null);
  assert.ok(session.branch, 'branch still named for later promotion');
}));

test('archiveSession saves a dirty worktree as a patch and keeps the branch', withEnv(async (env) => {
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  writeFileSync(join(session.worktreePath!, 'dirty.txt'), 'uncommitted work\n');
  const archived = await archiveSession('p', session.id, env.profileInfo);

  assert.ok(archived.archivedAt !== null, 'marked archived');
  assert.equal(archived.worktreePath, null);
  assert.ok(!existsSync(session.worktreePath!), 'worktree removed');
  const branches = execFileSync('git', ['branch', '--list', session.branch], { cwd: env.checkout, encoding: 'utf8' });
  assert.ok(branches.includes(session.branch), 'branch survives archive');

  const sessionDir = join(env.stateDir, 'project-p', `session-${session.id}`);
  const patches = readdirSync(sessionDir).filter((f) => f.endsWith('.patch'));
  assert.equal(patches.length, 1, 'exactly one patch');
  const patch = readFileSync(join(sessionDir, patches[0]), 'utf8');
  assert.match(patch, /dirty\.txt/, 'patch captures the uncommitted file');
}));

test('archiveSession of a clean worktree writes no patch', withEnv(async (env) => {
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  await archiveSession('p', session.id, env.profileInfo);
  const sessionDir = join(env.stateDir, 'project-p', `session-${session.id}`);
  assert.ok(!existsSync(sessionDir) || readdirSync(sessionDir).every((f) => !f.endsWith('.patch')));
}));

test('resolveSessionCwd rematerializes a reclaimed worktree from the branch', withEnv(async (env) => {
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  // Simulate `gah prune` reclaiming the idle clean worktree.
  execFileSync('git', ['worktree', 'remove', session.worktreePath!], { cwd: env.checkout });
  assert.ok(!existsSync(session.worktreePath!));

  const resolved = await resolveSessionCwd('p', session.id, env.profileInfo);
  assert.ok(resolved, 'session resolves after reclamation');
  assert.equal(resolved!.cwd, session.worktreePath);
  assert.ok(existsSync(session.worktreePath!), 'worktree rematerialized');

  // A committed branch change survives the round trip.
  writeFileSync(join(session.worktreePath!, 'kept.txt'), 'branch state\n');
  execFileSync('git', ['add', '.'], { cwd: session.worktreePath! });
  execFileSync('git', ['commit', '--quiet', '-m', 'work'], { cwd: session.worktreePath! });
  execFileSync('git', ['worktree', 'remove', session.worktreePath!], { cwd: env.checkout });
  const again = await resolveSessionCwd('p', session.id, env.profileInfo);
  assert.ok(existsSync(join(again!.cwd, 'kept.txt')), 'branch content restored');
}));

test('resolveSessionCwd rejects unknown and archived sessions', withEnv(async (env) => {
  assert.equal(await resolveSessionCwd('p', 'nope', env.profileInfo), null);
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  await archiveSession('p', session.id, env.profileInfo);
  assert.equal(await resolveSessionCwd('p', session.id, env.profileInfo), null);
}));

test('touchSession updates lastActiveAt only for live sessions', withEnv(async (env) => {
  const session = await createSession({ profile: 'p', profileInfo: env.profileInfo, backend: 'hermes' });
  const before = getSession('p', session.id)!.lastActiveAt;
  await new Promise((resolve) => setTimeout(resolve, 5));
  touchSession('p', session.id);
  assert.ok(getSession('p', session.id)!.lastActiveAt > before, 'activity recorded');
  touchSession('p', 'missing'); // must not throw
}));

test('chatKey keeps the bare profile for the default session', () => {
  assert.equal(chatKey('p'), 'p');
  assert.equal(chatKey('p', 'default'), 'p');
  assert.equal(chatKey('p', 'abc123'), 'p#abc123');
});

test('createSession stores the pinned model; updateSession switches backend/model in place', withEnv(async (env) => {
  const session = await createSession({
    profile: 'p',
    profileInfo: env.profileInfo,
    backend: 'hermes',
    model: 'mock-strong'
  });
  assert.equal(session.model, 'mock-strong');

  // Switching model/backend never touches the worktree or the branch.
  const switched = updateSession('p', session.id, { backend: 'codex', model: null });
  assert.equal(switched.backend, 'codex');
  assert.equal(switched.model, null);
  assert.equal(switched.worktreePath, session.worktreePath);
  assert.equal(switched.branch, session.branch);

  // Unknown or archived sessions refuse updates loudly.
  assert.throws(() => updateSession('p', 'missing', { backend: 'codex' }), /No chat session/);
  await archiveSession('p', session.id, env.profileInfo);
  assert.throws(() => updateSession('p', session.id, { backend: 'hermes' }), /archived/);
}));
