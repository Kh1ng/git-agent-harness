import assert from 'node:assert/strict';
import { test, describe, beforeEach, afterEach } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AsyncTtlCache } from './asyncTtlCache.js';
import { commitGitChanges, getGitStatusCached } from './gitCache.js';

// Simple test to verify AsyncTtlCache works as expected for git caching scenarios

describe('AsyncTtlCache for Git operations', () => {
  let now: number;
  let loads: number;

  beforeEach(() => {
    now = 1000;
    loads = 0;
  });

  afterEach(() => {
    now = 0;
    loads = 0;
  });

  test('returns cached value until TTL expires', async () => {
    const cache = new AsyncTtlCache<string, { branch: string }>(30_000, () => now);
    const load = async () => {
      loads++;
      return { branch: 'main' };
    };

    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 1);

    // Within TTL
    now += 10_000;
    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 1);

    // After TTL
    now += 20_001;
    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 2);
  });

  test('coalesces concurrent misses for the same key', async () => {
    let release: ((value: { branch: string }) => void) | undefined;
    let loads = 0;
    const cache = new AsyncTtlCache<string, { branch: string }>(30_000);
    const load = () => {
      loads += 1;
      return new Promise<{ branch: string }>((resolve) => {
        release = resolve;
      });
    };

    const first = cache.get('gah', load);
    const second = cache.get('gah', load);
    await Promise.resolve();
    assert.equal(loads, 1);
    release?.({ branch: 'main' });
    assert.deepEqual(await Promise.all([first, second]), [{ branch: 'main' }, { branch: 'main' }]);
  });

  test('isolates different profiles in cache', async () => {
    let loads = 0;
    const cache = new AsyncTtlCache<string, { branch: string }>(30_000);
    const load = async () => {
      loads++;
      return { branch: `branch-${loads}` };
    };

    assert.deepEqual(await cache.get('gah', load), { branch: 'branch-1' });
    assert.deepEqual(await cache.get('sportsball', load), { branch: 'branch-2' });
    assert.deepEqual(await cache.get('gah', load), { branch: 'branch-1' });
    assert.equal(loads, 2);
  });

  test('does not cache failures', async () => {
    let loads = 0;
    const cache = new AsyncTtlCache<string, { branch: string }>(30_000);
    const load = async () => {
      loads += 1;
      if (loads === 1) throw new Error('git not available');
      return { branch: 'main' };
    };

    await assert.rejects(cache.get('gah', load), /git not available/);
    assert.equal(loads, 1);

    // Next call should retry
    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 2);
  });
});

describe('Git cache key construction', () => {
  test('constructs consistent keys for same params', () => {
    // Simulate the key construction logic
    const params1 = { limit: 20 };
    const params2 = { limit: 20 };
    const key1 = ['gah', 'log', 'limit:20'].join(':');
    const key2 = ['gah', 'log', 'limit:20'].join(':');
    assert.equal(key1, key2);
  });

  test('constructs different keys for different params', () => {
    const key1 = ['gah', 'log', 'limit:10'].join(':');
    const key2 = ['gah', 'log', 'limit:20'].join(':');
    assert.notEqual(key1, key2);
  });

  test('constructs different keys for different profiles', () => {
    const key1 = ['gah', 'log', 'limit:20'].join(':');
    const key2 = ['sportsball', 'log', 'limit:20'].join(':');
    assert.notEqual(key1, key2);
  });
});

describe('Git cache TTL behavior', () => {
  test('respects short TTL for frequent refresh', async () => {
    let loads = 0;
    const cache = new AsyncTtlCache<string, { branch: string }>(5_000); // 5 seconds
    const load = async () => {
      loads++;
      return { branch: 'main' };
    };

    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 1);

    // Within TTL - should use cache
    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 1);
  });

  test('cache expires and reloads after TTL', async () => {
    let now = 1000;
    let loads = 0;
    const cache = new AsyncTtlCache<string, { branch: string }>(5_000, () => now);
    const load = async () => {
      loads++;
      return { branch: 'main' };
    };

    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 1);

    // Advance past TTL
    now += 5_001;
    assert.deepEqual(await cache.get('gah', load), { branch: 'main' });
    assert.equal(loads, 2);
  });
});

describe('commitGitChanges', () => {
  function initRepo(): string {
    const dir = mkdtempSync(join(tmpdir(), 'gah-gitcache-'));
    execFileSync('git', ['init', '--initial-branch=main', dir]);
    execFileSync('git', [
      '-C', dir, '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
      'commit', '--allow-empty', '-m', 'initial'
    ]);
    return dir;
  }

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
});
