import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { getCoordinatorIdentity, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';

const identityPath = (dir: string) => join(dir, 'coordinator-identity.json');

test('a first identity is generated and persisted; a valid file is reused as-is', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-identity-'));
  try {
    resetCachedCoordinatorIdentity();
    const first = getCoordinatorIdentity(identityPath(dir), 3773);
    assert.match(first.node_id, /^[0-9a-f-]{36}$/, 'must persist a generated node_id');
    assert.equal(first.display_name, 'GAH Coordinator');
    assert.equal(first.advertised_url, 'http://localhost:3773');
    const onDisk = JSON.parse(readFileSync(identityPath(dir), 'utf8'));
    assert.equal(onDisk.node_id, first.node_id, 'the written file is the source of truth');

    resetCachedCoordinatorIdentity();
    const reread = getCoordinatorIdentity(identityPath(dir), 3773);
    assert.equal(reread.node_id, first.node_id, 'an existing identity must be reused, not regenerated');

    writeFileSync(identityPath(dir), JSON.stringify({ node_id: 'abc-123', display_name: 'Named', advertised_url: 'https://central.example' }));
    resetCachedCoordinatorIdentity();
    const honored = getCoordinatorIdentity(identityPath(dir), 3773);
    assert.equal(honored.node_id, 'abc-123');
    assert.equal(honored.display_name, 'Named');
    assert.equal(honored.advertised_url, 'https://central.example');
  } finally {
    resetCachedCoordinatorIdentity();
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a corrupt identity file yields a fresh identity without throwing or overwriting the file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-identity-'));
  try {
    writeFileSync(identityPath(dir), '{broken');
    resetCachedCoordinatorIdentity();
    const identity = getCoordinatorIdentity(identityPath(dir), 3773);
    assert.match(identity.node_id, /^[0-9a-f-]{36}$/, 'a random id must be minted, not guessed from the corrupt file');
    assert.equal(readFileSync(identityPath(dir), 'utf8'), '{broken', 'the corrupt file must not be silently rewritten');
  } finally {
    resetCachedCoordinatorIdentity();
    rmSync(dir, { recursive: true, force: true });
  }
});

test('an unwritable identity directory never blocks identity resolution', () => {
  if (process.platform === 'win32' || process.getuid?.() === 0) return; // chmod is not a write barrier on Windows/root
  const dir = mkdtempSync(join(tmpdir(), 'gah-identity-'));
  const nested = join(dir, 'config');
  try {
    writeFileSync(join(dir, 'holder'), ''); // dir is non-empty so rmdir in cleanup is safe
    chmodSync(dir, 0o500);
    resetCachedCoordinatorIdentity();
    const identity = getCoordinatorIdentity(join(nested, 'coordinator-identity.json'), 3773);
    assert.match(identity.node_id, /^[0-9a-f-]{36}$/, 'the swallowed write failure must still leave a usable identity');
    assert.equal(existsSync(nested), false, 'nothing may be created inside the unwritable directory');
  } finally {
    chmodSync(dir, 0o700);
    resetCachedCoordinatorIdentity();
    rmSync(dir, { recursive: true, force: true });
  }
});

test('identities are cached per path and port until the cache is reset', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-identity-'));
  try {
    resetCachedCoordinatorIdentity();
    const a = getCoordinatorIdentity(identityPath(dir), 3773);
    const cached = getCoordinatorIdentity(identityPath(dir), 3773);
    assert.equal(cached, a, 'same path and port must reuse the cached identity object');
    const otherPort = getCoordinatorIdentity(identityPath(dir), 3774);
    assert.notEqual(otherPort, a, 'a different port is a different cache key');
    // The persisted file is the source of truth: its advertised_url wins
    // over the port argument, so an operator-edited address is never
    // clobbered by a restart that passes a different port.
    assert.equal(otherPort.node_id, a.node_id);
    assert.equal(otherPort.advertised_url, 'http://localhost:3773');

    writeFileSync(identityPath(dir), JSON.stringify({ node_id: 'changed' }));
    resetCachedCoordinatorIdentity();
    assert.equal(getCoordinatorIdentity(identityPath(dir), 3773).node_id, 'changed', 'reset must force a re-read');
  } finally {
    resetCachedCoordinatorIdentity();
    rmSync(dir, { recursive: true, force: true });
  }
});
