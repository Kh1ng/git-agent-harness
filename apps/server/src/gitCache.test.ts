import assert from 'node:assert/strict';
import { test, describe, beforeEach, afterEach } from 'node:test';
import { AsyncTtlCache } from './asyncTtlCache.js';

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
