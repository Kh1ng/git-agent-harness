import assert from 'node:assert/strict';
import { test } from 'node:test';
import { join } from 'node:path';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';

import {
  buildConfigSetArgs,
  buildProfileAddArgs,
  buildProfileSetArgs,
  clearManualStop,
  findGahBinary,
  loopManualStopFile,
  loopStateDir,
} from './gahCli.js';

test('config set args deduplicate clear values and use the CLI config flag', () => {
  assert.deepEqual(
    buildConfigSetArgs({
      current_manager: null,
      clear: ['current_manager', 'current_manager', 'other', 'other'],
      config: '/tmp/gah-config.toml',
    }),
    [
      'config',
      'set',
      '--clear',
      'current_manager',
      '--clear',
      'other',
      '--config-path',
      '/tmp/gah-config.toml',
    ],
  );
});

test('profile add args map required and optional fields without spawning gah', () => {
  assert.deepEqual(
    buildProfileAddArgs({
      name: 'api-worker',
      display_name: 'API Worker',
      repo_id: 'api-worker',
      provider: 'gitlab',
      repo: 'team/api-worker',
      local_path: '/srv/api-worker',
      artifact_root: '/srv/artifacts/api-worker',
      default_target_branch: 'trunk',
      validation_commands: ['cargo test', 'npm test'],
      max_parallel_workers: 2,
      validation_timeout_seconds: 900,
      manager_wake_autonomy: 'review_only',
      config: '/tmp/gah-config.toml',
    }),
    [
      'profile',
      'add',
      'api-worker',
      '--display-name',
      'API Worker',
      '--repo-id',
      'api-worker',
      '--provider',
      'gitlab',
      '--repo',
      'team/api-worker',
      '--local-path',
      '/srv/api-worker',
      '--artifact-root',
      '/srv/artifacts/api-worker',
      '--default-target-branch',
      'trunk',
      '--validation-commands',
      'cargo test,npm test',
      '--max-parallel-workers',
      '2',
      '--validation-timeout-seconds',
      '900',
      '--manager-wake-autonomy',
      'review_only',
      '--config',
      '/tmp/gah-config.toml',
    ],
  );
});

test('profile set args map fields and emit each clear key once', () => {
  assert.deepEqual(
    buildProfileSetArgs({
      name: 'api-worker',
      provider: 'github',
      max_parallel_workers: null,
      validation_timeout_seconds: 900,
      clear: [
        'max_parallel_workers',
        'max_parallel_workers',
        'manager_wake_autonomy',
        'other',
        'other',
      ],
      config: '/tmp/gah-config.toml',
    }),
    [
      'profile',
      'set',
      'api-worker',
      '--provider',
      'github',
      '--clear',
      'max_parallel_workers',
      '--validation-timeout-seconds',
      '900',
      '--clear',
      'manager_wake_autonomy',
      '--clear',
      'other',
      '--config',
      '/tmp/gah-config.toml',
    ],
  );
});

test('profile set emits validation timeout clear exactly once', () => {
  assert.deepEqual(
    buildProfileSetArgs({
      name: 'api-worker',
      validation_timeout_seconds: null,
      clear: ['validation_timeout_seconds', 'validation_timeout_seconds'],
    }),
    [
      'profile',
      'set',
      'api-worker',
      '--clear',
      'validation_timeout_seconds',
    ],
  );
});

test('loop state follows the resolved config directory', () => {
  assert.equal(
    loopStateDir('/srv/gah/config.toml', {}),
    join('/srv/gah', '.gah-locks'),
  );
});

test('loop state fallback order is XDG_STATE_HOME then HOME then tmp', () => {
  assert.equal(
    loopStateDir(null, { XDG_STATE_HOME: '/state', HOME: '/home/operator' }),
    join('/state', 'gah'),
  );
  assert.equal(
    loopStateDir(null, { HOME: '/home/operator' }),
    join('/home/operator', '.local/state/gah'),
  );
  assert.equal(loopStateDir(null, {}), join('/tmp', 'gah'));
});

test('manual-stop marker is written under the resolved loop state dir and cleared by clearManualStop', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-manual-stop-'));
  const originalConfig = process.env.GAH_CONFIG_PATH;
  const originalState = process.env.XDG_STATE_HOME;
  try {
    // No config path resolves -> fallback to XDG_STATE_HOME/gah.
    process.env.GAH_CONFIG_PATH = join(dir, 'nonexistent', 'config.toml');
    process.env.XDG_STATE_HOME = dir;
    delete process.env.GAH_CONFIG;

    const marker = loopManualStopFile('gah');
    assert.equal(marker, join(dir, 'gah', 'loop-gah.manual-stop.json'));

    mkdirSync(join(dir, 'gah'), { recursive: true });
    writeFileSync(marker, JSON.stringify({ stoppedAt: new Date().toISOString() }));
    assert.ok(existsSync(marker), 'marker should exist after a dashboard Stop');

    clearManualStop('gah');
    assert.ok(!existsSync(marker), 'Start must clear the marker so the loop resumes');
  } finally {
    if (originalConfig === undefined) delete process.env.GAH_CONFIG_PATH;
    else process.env.GAH_CONFIG_PATH = originalConfig;
    if (originalState === undefined) delete process.env.XDG_STATE_HOME;
    else process.env.XDG_STATE_HOME = originalState;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('manual-stop marker is profile-scoped', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-manual-stop-'));
  const originalState = process.env.XDG_STATE_HOME;
  try {
    process.env.XDG_STATE_HOME = dir;
    mkdirSync(join(dir, 'gah'), { recursive: true });
    writeFileSync(
      join(dir, 'gah', 'loop-sportsball.manual-stop.json'),
      JSON.stringify({ stoppedAt: new Date().toISOString() }),
    );
    // A different profile's marker must not be cleared.
    clearManualStop('gah');
    assert.ok(
      existsSync(join(dir, 'gah', 'loop-sportsball.manual-stop.json')),
      'clearing one profile must not clear another profile marker',
    );
  } finally {
    if (originalState === undefined) delete process.env.XDG_STATE_HOME;
    else process.env.XDG_STATE_HOME = originalState;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('gah binary resolution probes candidates in order and falls back to PATH', () => {
  const visited: string[] = [];
  const selected = findGahBinary((candidate) => {
    visited.push(candidate);
    return candidate.endsWith('/target/debug/gah');
  });

  assert.match(selected, /\/target\/debug\/gah$/);
  assert.equal(visited.length, 2);

  const unavailable: string[] = [];
  assert.equal(
    findGahBinary((candidate) => {
      unavailable.push(candidate);
      return false;
    }),
    'gah',
  );
  assert.equal(unavailable.at(-1), 'gah');
});

// Issue #635: GAH_BINARY lets tests/deployments pin a specific binary
// (a fixture, or a specific installed build) deterministically.
test('GAH_BINARY override is tried first, ahead of every build-path/PATH candidate', () => {
  const original = process.env.GAH_BINARY;
  process.env.GAH_BINARY = '/fixtures/gah';
  try {
    const visited: string[] = [];
    const selected = findGahBinary((candidate) => {
      visited.push(candidate);
      return candidate === '/fixtures/gah';
    });

    assert.equal(selected, '/fixtures/gah');
    assert.deepEqual(visited, ['/fixtures/gah']);
  } finally {
    if (original === undefined) delete process.env.GAH_BINARY;
    else process.env.GAH_BINARY = original;
  }
});

test('a missing or non-executable GAH_BINARY override falls through to normal resolution, not an error', () => {
  const original = process.env.GAH_BINARY;
  process.env.GAH_BINARY = '/fixtures/does-not-exist';
  try {
    const visited: string[] = [];
    const selected = findGahBinary((candidate) => {
      visited.push(candidate);
      return candidate.endsWith('/target/debug/gah');
    });

    assert.equal(visited[0], '/fixtures/does-not-exist');
    assert.match(selected, /\/target\/debug\/gah$/);
  } finally {
    if (original === undefined) delete process.env.GAH_BINARY;
    else process.env.GAH_BINARY = original;
  }
});

test('with no GAH_BINARY set, resolution is unaffected by the override candidate', () => {
  const original = process.env.GAH_BINARY;
  delete process.env.GAH_BINARY;
  try {
    const visited: string[] = [];
    const selected = findGahBinary((candidate) => {
      visited.push(candidate);
      return false;
    });

    assert.equal(selected, 'gah');
    assert.match(visited[0], /\/target\/release\/gah$/);
  } finally {
    if (original === undefined) delete process.env.GAH_BINARY;
    else process.env.GAH_BINARY = original;
  }
});
