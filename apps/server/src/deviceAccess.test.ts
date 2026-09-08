import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DeviceAccess, DEVICE_LIFETIME } from './deviceAccess.js';

test('pairing codes are single-use, origin/server-bound, expired and invalid after restart; disk keeps only hashes', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-pairing-'));
  const path = join(directory, 'devices.json');
  let now = Date.now();
  const server = { id: 'central-id', name: 'Test central', origin: 'https://gah.test' };
  try {
    const access = new DeviceAccess(path, () => now);
    const offer = access.create(server);
    assert.equal(access.inspect(offer.code, server.id, server.origin).server.id, server.id);
    assert.throws(() => access.redeem(offer.code, 'wrong-server', server.origin, 'Phone'), /another server/);
    assert.throws(() => access.redeem(offer.code, server.id, 'https://wrong.test', 'Phone'), /another server/);
    assert.throws(() => access.redeem(offer.code, server.id, server.origin, ''), /device name/);
    const first = access.redeem(offer.code, server.id, server.origin, 'Phone');
    assert.throws(() => access.redeem(offer.code, server.id, server.origin, 'Replay'), /already used/);
    assert.equal(access.authenticate(first.token)?.name, 'Phone');
    const disk = readFileSync(path, 'utf8');
    assert.ok(!disk.includes(first.token.split('.')[1]));
    assert.ok(!disk.includes(offer.code));
    assert.match(disk, /token_hash/);
    assert.ok(!JSON.stringify(access.list()).includes('token_hash'));
    if (process.platform !== 'win32') assert.equal(statSync(path).mode & 0o777, 0o600);
    const pending = access.create(server);
    const restarted = new DeviceAccess(path, () => now);
    assert.throws(() => restarted.inspect(pending.code, server.id, server.origin), /expired/);
    assert.equal(restarted.authenticate(first.token)?.id, first.device.id);
    now += 5 * 60_000;
    assert.throws(() => access.inspect(pending.code, server.id, server.origin), /expired/);
    const secondOffer = access.create(server);
    const second = access.redeem(secondOffer.code, server.id, server.origin, 'Tablet');
    const revoked: string[] = [];
    access.onRevoke(id => revoked.push(id));
    access.revoke(first.device.id);
    assert.deepEqual(revoked, [first.device.id]);
    assert.equal(access.authenticate(first.token), null);
    assert.equal(new DeviceAccess(path, () => now).authenticate(first.token), null);
    assert.ok(access.authenticate(second.token));
    now += DEVICE_LIFETIME;
    assert.equal(access.authenticate(second.token), null);
    writeFileSync(path, '{broken');
    assert.throws(() => new DeviceAccess(path).authenticate(second.token), /storage is invalid/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
