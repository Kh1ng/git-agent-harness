// Exercise packaging with fake binaries; no native builds or personal repository writes.
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';

const stageScript = resolve('scripts/stage-node-test-artifacts.mjs');
const root = mkdtempSync(join(tmpdir(), 'gah-artifact-stage-'));
const run = (command, args) => execFileSync(command, args, { cwd: root, stdio: 'pipe' });
try {
  for (const directory of ['scripts', 'target/release', 'apps/desktop/target/release/bundle/nsis']) mkdirSync(join(root, directory), { recursive: true });
  for (const name of ['install-windows.ps1', 'install-wsl-worker.sh']) writeFileSync(join(root, 'scripts', name), 'tracked installer');
  run('git', ['init']);
  run('git', ['add', 'scripts']);
  run('git', ['-c', 'user.name=Artifact test', '-c', 'user.email=artifact-test@example.invalid', 'commit', '-m', 'test source']);
  writeFileSync(join(root, 'scripts/install-windows.ps1'), 'checkout CRLF\r\n');
  writeFileSync(join(root, 'private-untracked.txt'), 'must not ship');
  writeFileSync(join(root, 'target/release/gah'), 'fake Linux binary');
  const exe = 'GAH Worker_0.1.1_x64-setup.exe';
  writeFileSync(join(root, 'apps/desktop/target/release/bundle/nsis', exe), 'fake Windows installer');
  const manifests = [];
  for (const kind of ['worker', 'desktop']) {
    run(process.execPath, [stageScript, kind, kind]);
    const manifest = JSON.parse(readFileSync(join(root, kind, `${kind}-artifact.json`), 'utf8'));
    assert.equal(manifest.revision, run('git', ['rev-parse', 'HEAD']).toString().trim());
    assert.equal(Object.keys(manifest.files).length, kind === 'worker' ? 4 : 2);
    for (const [name, digest] of Object.entries(manifest.files)) assert.equal(digest, createHash('sha256').update(readFileSync(join(root, kind, name))).digest('hex'));
    manifests.push(manifest);
  }
  assert.equal(readFileSync(join(root, 'desktop/install-windows.ps1'), 'utf8'), 'tracked installer');
  assert.equal(manifests[0].revision, manifests[1].revision);
  assert.equal(manifests[0].files['install-windows.ps1'], manifests[1].files['install-windows.ps1']);
  const archive = run('tar', ['-tzf', join(root, 'worker/source.tar.gz')]).toString();
  assert.match(archive, /scripts\/install-windows.ps1/);
  assert.ok(!archive.includes('private-untracked'));
  writeFileSync(join(root, 'apps/desktop/target/release/bundle/nsis', 'GAH Worker_0.1.2_x64-setup.exe'), 'another binary');
  assert.throws(() => run(process.execPath, [stageScript, 'desktop', 'ambiguous']));
  console.log('Artifact staging passed: matching revisions, file checksums, tracked source only, ambiguous installers rejected.');
} finally { rmSync(root, { recursive: true, force: true }); }
