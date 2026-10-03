import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = readFileSync(new URL('./build-signed-macos.sh', import.meta.url), 'utf8');
const secrets = {
  APPLE_CERTIFICATE: 'test-certificate', APPLE_CERTIFICATE_PASSWORD: 'test-password',
  APPLE_SIGNING_IDENTITY: 'Developer ID Application: Test (TEAM)',
  APPLE_ID: 'test@example.test', APPLE_PASSWORD: 'test-notary-password', APPLE_TEAM_ID: 'TEAM',
};

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'gah-signing-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const dir of ['scripts', 'bin', 'apps/desktop/target/release/bundle/macos/GAH.app', 'apps/desktop/target/release/bundle/dmg']) {
    mkdirSync(join(root, dir), { recursive: true });
  }
  const entry = join(root, 'scripts/build-signed-macos.sh');
  const log = join(root, 'commands.log');
  writeFileSync(entry, script);
  writeFileSync(join(root, 'apps/desktop/target/release/bundle/dmg/GAH.dmg'), 'fixture');
  for (const cmd of ['npx', 'codesign', 'spctl', 'xcrun']) {
    writeFileSync(join(root, 'bin', cmd), '#!/bin/bash\necho "$(basename "$0") $*" >> "$PROOF_LOG"\nif [ "$1" = "stapler" ] && [ "$2" = "validate" ] && [ "$FAIL_STAPLE" = "1" ]; then exit 9; fi\n', { mode: 0o755 });
  }
  const env = { ...process.env, ...secrets, PATH: `${join(root, 'bin')}:${process.env.PATH}`, PROOF_LOG: log, FAIL_STAPLE: '0' };
  const run = () => spawnSync('bash', [entry], { env, encoding: 'utf8' });
  return { env, run, log };
}

test('missing secrets and an ad-hoc identity stop before any build command', t => {
  const ctx = fixture(t);
  for (const name of Object.keys(secrets)) {
    const saved = ctx.env[name];
    ctx.env[name] = '';
    const result = ctx.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, new RegExp(name));
    assert.ok(!result.stderr.includes('test-password'));
    ctx.env[name] = saved;
  }
  ctx.env.APPLE_SIGNING_IDENTITY = '-';
  assert.equal(ctx.run().status, 1);
});

test('verifies the app and signs off the DMG only after notarization and stapling', t => {
  const ctx = fixture(t);
  assert.equal(ctx.run().status, 0);
  const calls = readFileSync(ctx.log, 'utf8');
  assert.match(calls, /npx tauri build --bundles dmg/);
  assert.match(calls, /codesign --verify --deep --strict .*GAH.app/);
  assert.match(calls, /notarytool submit .*GAH.dmg.*--wait/);
  assert.match(calls, /stapler staple .*GAH.dmg\nxcrun stapler validate .*GAH.dmg\nspctl --assess --type open/);
  ctx.env.FAIL_STAPLE = '1';
  assert.equal(ctx.run().status, 9, 'an invalid staple blocks the release');
});
