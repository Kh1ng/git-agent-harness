import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import {
  buildConfigSetArgs,
  buildProfileAddArgs,
  buildProfileSetArgs,
  findGahBinary,
  loopSystemctlArgs,
  runDispatchCancellable,
  startLoop,
  stopLoop,
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

test('loop lifecycle uses systemd enablement as the durable boot policy', () => {
  assert.deepEqual(loopSystemctlArgs(true, 'sportsball'), [
    '--user', 'enable', '--now', 'gah-loop@sportsball.service', '--no-pager'
  ]);
  assert.deepEqual(loopSystemctlArgs(false, 'sportsball'), [
    '--user', 'disable', '--now', 'gah-loop@sportsball.service', '--no-pager'
  ]);
});

test('stop disables an enabled unit even when it is already inactive', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-stop-loop-'));
  const log = join(dir, 'systemctl.log');
  const originalPath = process.env.PATH;
  process.env.PATH = `${dir}:${originalPath ?? ''}`;
  process.env.SYSTEMCTL_LOG = log;
  writeFileSync(join(dir, 'systemctl'), `#!/bin/sh
if [ "$2" = "show" ]; then
  printf 'LoadState=loaded\nActiveState=failed\nMainPID=0\n'
  exit 0
fi
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
`);
  writeFileSync(join(dir, 'pgrep'), '#!/bin/sh\nexit 1\n');
  chmodSync(join(dir, 'systemctl'), 0o755);
  chmodSync(join(dir, 'pgrep'), 0o755);
  try {
    assert.deepEqual(stopLoop('sportsball'), { stopped: true });
    assert.match(readFileSync(log, 'utf8'), /disable --now gah-loop@sportsball\.service/);
  } finally {
    if (originalPath === undefined) delete process.env.PATH;
    else process.env.PATH = originalPath;
    delete process.env.SYSTEMCTL_LOG;
    rmSync(dir, { recursive: true, force: true });
  }
});

/** A failed `enable --now` must not un-enable a unit that was already
 * enabled (boot-persisted) -- that would break its next-boot start, the exact
 * regression this branch exists to fix. The fake systemctl reports prior
 * enablement via FAKE_IS_ENABLED and always fails the enable --now activation. */
async function assertStartRollbackBehavior(wasEnabled: string, expectDisableInLog: boolean) {
  const dir = mkdtempSync(join(tmpdir(), 'gah-start-loop-'));
  const log = join(dir, 'systemctl.log');
  const originalPath = process.env.PATH;
  process.env.PATH = `${dir}:${originalPath ?? ''}`;
  process.env.SYSTEMCTL_LOG = log;
  process.env.FAKE_IS_ENABLED = wasEnabled;
  writeFileSync(join(dir, 'systemctl'), `#!/bin/sh
case "$2" in
  show)
    printf 'LoadState=loaded\nActiveState=failed\nMainPID=0\n'
    exit 0
    ;;
  is-enabled)
    if [ "$FAKE_IS_ENABLED" = "enabled" ]; then echo enabled; exit 0; fi
    echo disabled; exit 1
    ;;
  enable)
    printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
    echo "Failed to activate" >&2
    exit 1
    ;;
  *)
    printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
    exit 0
    ;;
esac
`);
  writeFileSync(join(dir, 'pgrep'), '#!/bin/sh\nexit 1\n');
  chmodSync(join(dir, 'systemctl'), 0o755);
  chmodSync(join(dir, 'pgrep'), 0o755);
  try {
    const result = await startLoop('sportsball');
    assert.equal(result.started, false);
    assert.match(result.error ?? '', /Failed to activate/);
    const logText = readFileSync(log, 'utf8');
    if (expectDisableInLog) {
      assert.match(logText, /disable --now gah-loop@sportsball\.service/, 'rollback should disable a unit startLoop itself enabled');
    } else {
      assert.doesNotMatch(logText, /disable --now/, 'a previously-enabled unit must NOT be disabled by a failed start');
    }
  } finally {
    if (originalPath === undefined) delete process.env.PATH;
    else process.env.PATH = originalPath;
    delete process.env.SYSTEMCTL_LOG;
    delete process.env.FAKE_IS_ENABLED;
    rmSync(dir, { recursive: true, force: true });
  }
}

test('a failed start disables only a unit startLoop itself enabled', async () => {
  await assertStartRollbackBehavior('disabled', true);
});

test('a failed start does not disable a previously-enabled unit (boot-persisted loop survives)', async () => {
  await assertStartRollbackBehavior('enabled', false);
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

function isPidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function waitUntil(predicate: () => boolean, timeoutMs = 5000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error('Timed out waiting for condition');
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

// P0 regression: cancel() must kill the whole process tree, not just the
// direct `gah` child. GAH_BINARY here points at a fake "gah" that forks a
// grandchild (a background `sleep`) -- the real dispatch/validation
// processes GAH itself spawns. If cancel() only signalled the direct
// child (or used the unsupported SpawnOptions.setsid that never took
// effect), the grandchild would survive.
test(
  'cancel() confirms termination of the whole descendant process tree, not just the direct child',
  { skip: process.platform === 'win32' ? 'process-group signalling is Unix-only' : false },
  async () => {
    const dir = mkdtempSync(join(tmpdir(), 'gah-descendant-'));
    const script = join(dir, 'fake-gah');
    const marker = join(dir, 'grandchild.pid');
    writeFileSync(
      script,
      `#!/bin/sh
sleep 60 &
echo $! > "${marker}"
wait
`
    );
    chmodSync(script, 0o755);

    const original = process.env.GAH_BINARY;
    process.env.GAH_BINARY = script;
    try {
      const dispatch = runDispatchCancellable({ profile: 'p', mode: 'fix' }, () => {});

      await waitUntil(() => existsSync(marker));
      const grandchildPid = Number.parseInt(readFileSync(marker, 'utf8').trim(), 10);
      assert.ok(isPidAlive(grandchildPid), 'grandchild should be running before cancellation');

      const result = await dispatch.cancel();

      assert.deepEqual(result, { cancelled: true });
      assert.equal(
        isPidAlive(grandchildPid),
        false,
        'grandchild must be terminated along with its parent process group'
      );

      const dispatchResult = await dispatch.promise;
      assert.notEqual(dispatchResult.exitCode, 0);
    } finally {
      if (original === undefined) delete process.env.GAH_BINARY;
      else process.env.GAH_BINARY = original;
      rmSync(dir, { recursive: true, force: true });
    }
  }
);

// P0 regression: cancel() must not report success for a pid that never
// existed / already exited -- it has to return an explicit error rather
// than a bare "cancelled: true" it cannot actually back up.
test('cancel() returns an explicit error when the process has already completed', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-already-done-'));
  const script = join(dir, 'fake-gah');
  writeFileSync(script, `#!/bin/sh\nexit 0\n`);
  chmodSync(script, 0o755);

  const original = process.env.GAH_BINARY;
  process.env.GAH_BINARY = script;
  try {
    const dispatch = runDispatchCancellable({ profile: 'p', mode: 'fix' }, () => {});
    await dispatch.promise;

    const result = await dispatch.cancel();
    assert.equal(result.cancelled, false);
    assert.match(result.error ?? '', /already completed/);
  } finally {
    if (original === undefined) delete process.env.GAH_BINARY;
    else process.env.GAH_BINARY = original;
    rmSync(dir, { recursive: true, force: true });
  }
});

// Every caller of cancel() on the same dispatch must observe the exact
// same settlement, and the underlying kill/confirm sequence must run
// exactly once -- not once per caller.
test('cancel() is idempotent: concurrent callers settle on one shared result', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-idempotent-cancel-'));
  const script = join(dir, 'fake-gah');
  writeFileSync(script, `#!/bin/sh\nsleep 60\n`);
  chmodSync(script, 0o755);

  const original = process.env.GAH_BINARY;
  process.env.GAH_BINARY = script;
  try {
    const dispatch = runDispatchCancellable({ profile: 'p', mode: 'fix' }, () => {});
    await new Promise((resolve) => setTimeout(resolve, 50));

    const [first, second] = await Promise.all([dispatch.cancel(), dispatch.cancel()]);
    assert.deepEqual(first, { cancelled: true });
    assert.deepEqual(second, { cancelled: true });
  } finally {
    if (original === undefined) delete process.env.GAH_BINARY;
    else process.env.GAH_BINARY = original;
    rmSync(dir, { recursive: true, force: true });
  }
});
