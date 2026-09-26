import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  pushRegistrationId,
  removePushEntries,
  validPushDeviceLabel,
  validPushRegistrationId,
  writePrivatePushStore,
} from './pushStore.js';

test('writePrivatePushStore replaces the store atomically with owner-only permissions', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-push-store-'));
  const path = join(dir, 'push.json');
  try {
    writePrivatePushStore(path, [{ id: 'a' }]);
    writePrivatePushStore(path, [{ id: 'b' }]);
    const parsed = JSON.parse(readFileSync(path, 'utf8'));
    assert.equal(parsed[0].id, 'b', 'the second write must fully replace the first');
    if (process.platform !== 'win32') assert.equal(statSync(path).mode & 0o777, 0o600);
    assert.equal(readdirSync(dir).filter((name) => name.includes('.tmp-')).length, 0, 'no temporary files may survive a success');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('writePrivatePushStore cleans up its temporary file when the replace fails', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-push-store-'));
  const blockingDir = join(dir, 'push.json'); // a directory cannot be replaced by rename
  try {
    mkdirSync(blockingDir);
    assert.throws(() => writePrivatePushStore(blockingDir, []));
    assert.equal(
      readdirSync(dir).filter((name) => name.includes('.tmp-')).length,
      0,
      'the failed write must unlink its temporary file'
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('removePushEntries persists the remainder only when something was removed', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-push-store-'));
  const path = join(dir, 'push.json');
  try {
    const entries = [{ id: 'a', deviceId: 'd1' }, { id: 'b' }];
    writePrivatePushStore(path, entries);
    const before = readFileSync(path, 'utf8');

    const untouched = removePushEntries(path, entries, () => false);
    assert.equal(untouched.removed, false);
    assert.equal(readFileSync(path, 'utf8'), before, 'a no-op removal must not rewrite the store');

    const removed = removePushEntries(path, entries, (entry) => entry.id === 'a');
    assert.equal(removed.removed, true);
    assert.deepEqual(removed.remaining, [{ id: 'b' }]);
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), [{ id: 'b' }]);
    if (process.platform !== 'win32') assert.equal(statSync(path).mode & 0o777, 0o600, 'the rewrite must keep owner-only permissions');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('registration ids are deterministic, 24-hex validators accept only the real shape', () => {
  const id = pushRegistrationId('https://example.compush/abc');
  assert.equal(id, pushRegistrationId('https://example.compush/abc'), 'the same endpoint must map to one id');
  assert.match(id, /^[a-f0-9]{24}$/);
  assert.ok(validPushRegistrationId(id));
  assert.ok(!validPushRegistrationId('A'.repeat(24)), 'uppercase is not a hash output');
  assert.ok(!validPushRegistrationId(id.slice(0, 23)));
  assert.ok(!validPushRegistrationId(42));

  assert.ok(validPushDeviceLabel(undefined));
  assert.ok(validPushDeviceLabel('Phone'.repeat(16)), '80 characters is allowed');
  assert.ok(!validPushDeviceLabel('Phone'.repeat(16) + '!'), 'over 80 characters is rejected');
  assert.ok(!validPushDeviceLabel('bad\nlabel'), 'control characters are rejected');
});

test('the store directory is created with owner-only permissions when missing', () => {
  if (process.platform === 'win32') return;
  const dir = mkdtempSync(join(tmpdir(), 'gah-push-store-'));
  const nested = join(dir, 'nested', 'deeper');
  const path = join(nested, 'push.json');
  try {
    writePrivatePushStore(path, []);
    assert.equal(statSync(nested).mode & 0o777, 0o700, 'created directories must not be group-readable');
    assert.equal(statSync(path).mode & 0o777, 0o600);
  } finally {
    chmodSync(nested, 0o700); // in case the assert fired before cleanup
    rmSync(dir, { recursive: true, force: true });
  }
});
