import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = readFileSync(new URL('./compose-edge-manifest.mjs', import.meta.url), 'utf8');

/** The compose script runs from the downloaded-artifacts directory and reads
 * package.json there; the fixture recreates that layout. */
function fixture(t, { fragments = [] } = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'gah-edge-manifest-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, 'package.json'), '{"version":"0.1.3"}');
  for (const name of ['gah-linux-x86_64', 'gah-macos-universal', 'gah-mcp-server-linux-x86_64', 'gah-mcp-server-macos-universal', 'gah-server-bundle.tar.gz']) {
    writeFileSync(join(dir, name), `fixture-bytes-${name}`);
  }
  for (const [name, body] of fragments) {
    writeFileSync(join(dir, name), JSON.stringify(body));
  }
  const run = (headSha = '0123456789abcdef0123456789abcdef01234567') =>
    spawnSync(process.execPath, [join(dir, 'compose-edge-manifest.mjs'), headSha], {
      cwd: dir,
      encoding: 'utf8'
    });
  writeFileSync(join(dir, 'compose-edge-manifest.mjs'), script);
  return { dir, run };
}

test('composes edge-manifest.json with per-asset SHA-256 and no desktop feed when unsigned', t => {
  const { dir, run } = fixture(t);
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(join(dir, 'edge-manifest.json'), 'utf8'));
  assert.equal(manifest.schema, 1);
  assert.equal(manifest.version, '0.1.3');
  assert.equal(manifest.channel, 'edge');
  assert.equal(manifest.commit, '0123456789abcdef0123456789abcdef01234567');
  assert.equal(manifest.assets.length, 5);
  assert.deepEqual(
    manifest.assets.filter((asset) => asset.kind === 'mcp-server').map((asset) => asset.name),
    ['gah-mcp-server-linux-x86_64', 'gah-mcp-server-macos-universal']
  );
  const bundle = manifest.assets.find((asset) => asset.kind === 'server-bundle');
  assert.equal(bundle.name, 'gah-server-bundle.tar.gz');
  assert.match(bundle.sha256, /^[0-9a-f]{64}$/);
  assert.equal(bundle.url, 'https://github.com/Kh1ng/git-agent-harness/releases/download/edge/gah-server-bundle.tar.gz');
  for (const asset of manifest.assets) {
    assert.ok(asset.size > 0);
  }
  assert.equal(existsSync(join(dir, 'latest.json')), false, 'no signing key configured: no updater feed');
});

test('merges per-platform fragments into the signed latest.json feed', t => {
  const { dir, run } = fixture(t, {
    fragments: [
      ['fragment-macos.json', {
        platform_keys: ['darwin-aarch64', 'darwin-x86_64'],
        asset_name: 'GAH.app.tar.gz',
        signature: 'minisig-macos'
      }],
      ['fragment-windows.json', {
        platform_keys: ['windows-x86_64'],
        asset_name: 'GAH_0.1.3_x64-setup.exe',
        signature: 'minisig-windows'
      }]
    ]
  });
  writeFileSync(join(dir, 'GAH.app.tar.gz'), 'macos-bundle');
  writeFileSync(join(dir, 'GAH_0.1.3_x64-setup.exe'), 'windows-bundle');
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  const feed = JSON.parse(readFileSync(join(dir, 'latest.json'), 'utf8'));
  assert.equal(feed.version, '0.1.3');
  assert.ok(feed.pub_date);
  assert.deepEqual(Object.keys(feed.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64', 'windows-x86_64']);
  assert.equal(feed.platforms['darwin-aarch64'].signature, 'minisig-macos');
  assert.equal(
    feed.platforms['windows-x86_64'].url,
    'https://github.com/Kh1ng/git-agent-harness/releases/download/edge/GAH_0.1.3_x64-setup.exe'
  );
});

test('a fragment naming a missing asset fails the compose', t => {
  const { run } = fixture(t, {
    fragments: [['fragment-linux.json', { platform_keys: ['linux-x86_64'], asset_name: 'missing.AppImage', signature: 'sig' }]]
  });
  const result = run();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /missing updater asset: missing\.AppImage/);
});

test('a malformed head sha is refused before anything is written', t => {
  const { dir, run } = fixture(t);
  const result = run('not-a-sha');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /usage: compose-edge-manifest/);
  assert.equal(existsSync(join(dir, 'edge-manifest.json')), false);
});
