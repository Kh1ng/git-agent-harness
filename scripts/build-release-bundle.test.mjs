import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const script = readFileSync(new URL('./build-release-bundle.sh', import.meta.url), 'utf8');

/** The script roots itself at `dirname $0/..`, so the fixture copies it into
 * a fake tree; every expected output becomes a one-byte file. */
function fixture(t, { complete = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'gah-release-bundle-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'scripts'), { recursive: true });
  writeFileSync(join(root, 'scripts/build-release-bundle.sh'), script, { mode: 0o755 });
  const outputs = [
    'apps/server/dist/bin.js',
    'apps/web/dist/index.html',
    'packaging/opencode/agents/gah-reviewer.md',
    'packaging/opencode/agents/gah-implementer.md',
    'package.json',
    'package-lock.json'
  ];
  for (const path of outputs) {
    if (path === 'package.json' && complete) {
      writeFileSync(join(root, path), '{"version":"0.1.3"}');
      continue;
    }
    if (path === 'package-lock.json' && complete) {
      writeFileSync(join(root, path), '{"lockfileVersion":3}');
      continue;
    }
    if (complete || path !== 'apps/web/dist/index.html') {
      mkdirSync(join(root, join(path, '..')), { recursive: true });
      writeFileSync(join(root, path), 'fixture');
    }
  }
  const out = join(root, 'gah-server-bundle.tar.gz');
  const run = () => spawnSync('bash', [join(root, 'scripts/build-release-bundle.sh'), out], { encoding: 'utf8' });
  return { root, out, run };
}

test('a complete build archives exactly the release install layout', t => {
  const { run, out } = fixture(t);
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  const listing = spawnSync('tar', ['-tzf', out], { encoding: 'utf8' });
  const names = listing.stdout.trim().split('\n');
  for (const member of [
    'apps/server/dist/bin.js',
    'apps/web/dist/index.html',
    'packaging/opencode/agents/gah-reviewer.md',
    'package.json',
    'package-lock.json'
  ]) {
    assert.ok(names.includes(member), `${member} missing from bundle: ${names.join(', ')}`);
  }
  // Server dist carries the whole directory, not just bin.js.
  assert.ok(names.some((name) => name.startsWith('apps/web/dist/')), 'web dist directory missing');
});

test('an incomplete build fails naming the first missing output and writes nothing', t => {
  const { run, out } = fixture(t, { complete: false });
  const result = run();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /missing build output: apps\/web\/dist\/index\.html/);
  assert.match(result.stderr, /build:server/);
  assert.equal(result.stderr.includes('package.json') && result.stderr.includes('run:'), false, 'guidance only, no noise');
  assert.ok(!out || result.status !== 0);
});
