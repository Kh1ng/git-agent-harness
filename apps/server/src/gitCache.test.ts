import assert from 'node:assert/strict';
import { test, describe } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { commitGitChanges, getGitStatusCached } from './gitCache.js';

// AsyncTtlCache's own TTL/coalescing/isolation/failure behavior is covered
// by asyncTtlCache.test.ts. These tests focus on gitCache's own wrapper
// logic: status parsing, session-scoped keys, and the commit mutation.

function initRepo(): string {
  const dir = mkdtempSync(join(tmpdir(), 'gah-gitcache-'));
  execFileSync('git', ['init', '--initial-branch=main', dir]);
  // Repo-local config, not `-c` flags on a single invocation: commitGitChanges
  // runs a plain `git commit` with no `-c` flags of its own, so the identity
  // must be persisted in .git/config or it fails with "Author identity
  // unknown" on any machine/CI runner without a global user.name/user.email.
  execFileSync('git', ['-C', dir, 'config', 'user.name', 'Test']);
  execFileSync('git', ['-C', dir, 'config', 'user.email', 'test@example.com']);
  execFileSync('git', ['-C', dir, 'commit', '--allow-empty', '-m', 'initial']);
  return dir;
}

describe('getGitStatusCached', () => {
  test('strips the upstream tracking suffix from the branch name', async () => {
    const originDir = mkdtempSync(join(tmpdir(), 'gah-gitcache-origin-'));
    execFileSync('git', ['init', '--bare', '--initial-branch=main', originDir]);
    const dir = initRepo();
    execFileSync('git', ['-C', dir, 'remote', 'add', 'origin', originDir]);
    execFileSync('git', ['-C', dir, 'push', '-u', 'origin', 'main']);

    // `git status -b`'s header is `## main...origin/main`; the cached
    // result must report just `main`, not the raw tracking suffix.
    const status = await getGitStatusCached('gitcache-test-branch-suffix', dir);
    assert.equal(status.branch, 'main');
  });

  test('scopes the cache by session id so a session read is not served the profile-level entry', async () => {
    const dir = initRepo();
    const profile = 'gitcache-test-session-scope';

    const profileView = await getGitStatusCached(profile, dir);
    assert.equal(profileView.changes.length, 0);

    writeFileSync(join(dir, 'file.txt'), 'hello\n');
    const sessionView = await getGitStatusCached(profile, dir, 'session-1');
    assert.equal(sessionView.changes.length, 1);
  });
});

describe('commitGitChanges', () => {
  test('commits staged and unstaged changes and returns the new hash', async () => {
    const dir = initRepo();
    writeFileSync(join(dir, 'file.txt'), 'hello\n');

    const result = await commitGitChanges('gitcache-test-commit', dir, 'add file');

    assert.match(result.hash, /^[0-9a-f]{40}$/);
    const subject = execFileSync('git', ['-C', dir, 'log', '-1', '--pretty=%s'], { encoding: 'utf8' }).trim();
    assert.equal(subject, 'add file');
  });

  test('rejects when there is nothing to commit', async () => {
    const dir = initRepo();
    await assert.rejects(commitGitChanges('gitcache-test-empty', dir, 'no-op'));
  });

  test('invalidates the cached status so the next read reflects the commit', async () => {
    const dir = initRepo();
    const profile = 'gitcache-test-invalidate';
    writeFileSync(join(dir, 'file.txt'), 'hello\n');

    const dirty = await getGitStatusCached(profile, dir);
    assert.equal(dirty.changes.length, 1);

    await commitGitChanges(profile, dir, 'add file');
    const after = await getGitStatusCached(profile, dir);
    assert.equal(after.changes.length, 0);
  });

  test('invalidates the session-scoped cached status after a session commit', async () => {
    const dir = initRepo();
    const profile = 'gitcache-test-session-invalidate';
    const sessionId = 'session-1';
    writeFileSync(join(dir, 'file.txt'), 'hello\n');

    const dirty = await getGitStatusCached(profile, dir, sessionId);
    assert.equal(dirty.changes.length, 1);

    await commitGitChanges(profile, dir, 'add file', sessionId);
    const after = await getGitStatusCached(profile, dir, sessionId);
    assert.equal(after.changes.length, 0);
  });
});
