import assert from 'node:assert/strict';
import { test } from 'node:test';
import { AsyncTtlCache } from './asyncTtlCache.js';

test('returns a cached value until its TTL expires', async () => {
  let now = 1_000;
  let loads = 0;
  const cache = new AsyncTtlCache<string, number>(30_000, () => now);
  const load = async () => ++loads;

  assert.equal(await cache.get('gah', load), 1);
  now += 29_999;
  assert.equal(await cache.get('gah', load), 1);
  now += 1;
  assert.equal(await cache.get('gah', load), 2);
  assert.equal(loads, 2);
});

test('coalesces concurrent misses for the same key', async () => {
  let release: ((value: number) => void) | undefined;
  let loads = 0;
  const cache = new AsyncTtlCache<string, number>(30_000);
  const load = () => {
    loads += 1;
    return new Promise<number>((resolve) => {
      release = resolve;
    });
  };

  const first = cache.get('gah', load);
  const second = cache.get('gah', load);
  await Promise.resolve();
  assert.equal(loads, 1);
  release?.(42);
  assert.deepEqual(await Promise.all([first, second]), [42, 42]);
});

test('keeps profiles and config paths isolated by key', async () => {
  let loads = 0;
  const cache = new AsyncTtlCache<string, number>(30_000);
  const load = async () => ++loads;

  assert.equal(await cache.get('["gah",null]', load), 1);
  assert.equal(await cache.get('["sportsball",null]', load), 2);
  assert.equal(await cache.get('["gah","/tmp/other.toml"]', load), 3);
  assert.equal(await cache.get('["gah",null]', load), 1);
});

test('does not cache failures and retries the next caller', async () => {
  let loads = 0;
  const cache = new AsyncTtlCache<string, number>(30_000);
  const load = async () => {
    loads += 1;
    if (loads === 1) throw new Error('provider unavailable');
    return 7;
  };

  await assert.rejects(cache.get('gah', load), /provider unavailable/);
  assert.equal(await cache.get('gah', load), 7);
  assert.equal(loads, 2);
});
