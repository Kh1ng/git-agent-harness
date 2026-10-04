import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { codeqlScope } from './codeql-scope.mjs';

const selected = paths => codeqlScope(paths).include.map(row => row.language);

test('source and build inputs select only the affected language scans', () => {
  for (const [paths, expected] of [
    [['apps/ios/GAH/GAHApp.swift'], ['swift']],
    [['apps/ios/GAH.xcodeproj/project.pbxproj', 'apps/ios/GAH/Info.plist'], ['swift']],
    [['Package.resolved'], ['swift']],
    [['apps/server/src/quota.ts', 'apps/web/src/Page.tsx'], ['javascript-typescript']],
    [['scripts/build.mjs', 'package-lock.json', 'packages/shared/tsconfig.json'], ['javascript-typescript']],
    [['src/quota.rs', 'Cargo.lock'], ['rust']],
    [['apps/desktop/main.rs', 'apps/desktop/Cargo.toml', '.cargo/config.toml'], ['rust']],
    [['apps/android/app/src/main/java/MainActivity.java', 'apps/android/build.gradle'], ['java-kotlin']],
    [['apps/android/build.gradle.kts'], ['java-kotlin']],
    [['apps/ios/GAHTests/fixture.py'], ['python']],
    [['requirements-dev.txt'], ['python']],
    [['.github/workflows/CI.yml'], ['actions']],
    [['docs/README.md', 'scripts/install-linux.sh', 'apps/ios/Assets/icon.png'], []],
    [['src/quota.rs', 'apps/web/src/Page.tsx'], ['javascript-typescript', 'rust']],
    [[], []],
  ]) assert.deepEqual(selected(paths), expected, paths.join(', '));
});

test('scan configuration changes and periodic/manual runs select every language', () => {
  const all = ['actions', 'java-kotlin', 'javascript-typescript', 'python', 'rust', 'swift'];
  for (const path of ['.github/workflows/codeql.yml', '.github/codeql/config.yml', 'scripts/codeql-scope.mjs', 'scripts/codeql-scope.test.mjs']) {
    assert.deepEqual(selected([path]), all);
  }
  assert.deepEqual(codeqlScope([], true).include.map(row => row.language), all);
  assert.equal(codeqlScope(['apps/ios/GAH/GAHApp.swift']).include[0].os, 'macos-latest');
  assert.ok(codeqlScope(['src/quota.rs', 'apps/web/src/Page.tsx']).include.every(row => row.os === 'ubuntu-latest'));
});

test('CLI consumes NUL-separated paths and emits valid job outputs', () => {
  const result = spawnSync(process.execPath, ['scripts/codeql-scope.mjs'], {
    input: 'docs/a\nnew name.md\0apps/web/src/with space.ts\0src/deleted.rs\0', encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  const [matrix, hasLanguages] = result.stdout.trim().split('\n');
  assert.deepEqual(JSON.parse(matrix.slice('matrix='.length)), codeqlScope(['apps/web/src/with space.ts', 'src/deleted.rs']));
  assert.equal(hasLanguages, 'has_languages=true');
  const empty = spawnSync(process.execPath, ['scripts/codeql-scope.mjs'], { input: '', encoding: 'utf8' });
  assert.equal(empty.stdout, 'matrix={"include":[]}\nhas_languages=false\n');
});

test('workflow git diff excludes base-only changes and includes deleted/renamed languages', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-codeql-scope-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' }).trim();
  try {
    git('init', '-b', 'main');
    git('config', 'user.name', 'CI scope test');
    git('config', 'user.email', 'ci-scope@example.invalid');
    mkdirSync(join(dir, 'scripts'));
    copyFileSync('scripts/codeql-scope.mjs', join(dir, 'scripts/codeql-scope.mjs'));
    writeFileSync(join(dir, 'old.rs'), '// existing Rust source');
    writeFileSync(join(dir, 'old.swift'), '// existing Swift source');
    git('add', '.'); git('commit', '-m', 'base');
    git('checkout', '-b', 'pull-request');
    git('mv', 'old.rs', 'renamed.md');
    git('rm', 'old.swift');
    git('commit', '-am', 'rename Rust source and delete Swift source');
    const head = git('rev-parse', 'HEAD');
    git('checkout', 'main');
    writeFileSync(join(dir, 'base-only.ts'), '// unrelated new source on main');
    git('add', '.'); git('commit', '-m', 'advance base');
    const base = git('rev-parse', 'HEAD');
    const workflow = readFileSync('.github/workflows/codeql.yml', 'utf8');
    const script = workflow.split('- name: Select changed languages\n')[1]
      .split('      - name: Report migration status')[0]
      .split('        run: |\n')[1].split('\n').map(line => line.slice(10)).join('\n');
    const output = join(dir, 'output');
    const result = spawnSync('bash', ['-c', script], {
      cwd: dir, encoding: 'utf8',
      env: { ...process.env, EVENT_NAME: 'pull_request', BASE_SHA: base, HEAD_SHA: head, GITHUB_OUTPUT: output },
    });
    assert.equal(result.status, 0, result.stderr);
    const matrix = JSON.parse(readFileSync(output, 'utf8').split('\n')[0].slice('matrix='.length));
    assert.deepEqual(matrix.include.map(row => row.language), ['rust', 'swift']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
