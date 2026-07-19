import assert from 'node:assert/strict';
import { test } from 'node:test';
import { join } from 'node:path';

import {
  buildConfigSetArgs,
  buildProfileAddArgs,
  buildProfileSetArgs,
  findGahBinary,
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
