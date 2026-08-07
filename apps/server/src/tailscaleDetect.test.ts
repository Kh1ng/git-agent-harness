// Issue #880/#881 follow-up: detectTailscaleIPv4() must never throw and
// must degrade to null on anything short of a real, parseable
// `tailscale status --json` -- a real fake `tailscale` binary on PATH
// (not a mocked child_process), matching this repo's Rust-side PathGuard
// convention for faking subprocess dependencies.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, writeFileSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { detectTailscaleIPv4 } from './tailscaleDetect.js';

function withFakeTailscale(script: string, testFn: () => Promise<void>) {
  const dir = mkdtempSync(join(tmpdir(), 'gah-fake-tailscale-'));
  const binPath = join(dir, 'tailscale');
  writeFileSync(binPath, `#!/bin/sh\n${script}\n`);
  chmodSync(binPath, 0o755);

  const savedPath = process.env.PATH;
  process.env.PATH = `${dir}:${savedPath}`;
  return testFn().finally(() => {
    process.env.PATH = savedPath;
  });
}

test('parses the real tailscale status --json shape', async () => {
  await withFakeTailscale(
    `echo '{"Self":{"TailscaleIPs":["100.118.97.79","fd7a:115c:a1e0::7234:6151"]}}'`,
    async () => {
      assert.equal(await detectTailscaleIPv4(), '100.118.97.79');
    }
  );
});

test('returns null when tailscale is not on PATH at all', async () => {
  const savedPath = process.env.PATH;
  process.env.PATH = '/nonexistent-empty-dir';
  try {
    assert.equal(await detectTailscaleIPv4(), null);
  } finally {
    process.env.PATH = savedPath;
  }
});

test('returns null instead of throwing on a nonzero exit (e.g. not logged in)', async () => {
  await withFakeTailscale(`echo 'not logged in' >&2; exit 1`, async () => {
    assert.equal(await detectTailscaleIPv4(), null);
  });
});

test('returns null instead of throwing on malformed JSON', async () => {
  await withFakeTailscale(`echo 'not json'`, async () => {
    assert.equal(await detectTailscaleIPv4(), null);
  });
});

test('returns null when Self has no IPv4-looking address', async () => {
  await withFakeTailscale(`echo '{"Self":{"TailscaleIPs":["fd7a:115c:a1e0::7234:6151"]}}'`, async () => {
    assert.equal(await detectTailscaleIPv4(), null);
  });
});
