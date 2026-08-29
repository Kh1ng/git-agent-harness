import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChildProcess, spawn, spawnSync } from 'node:child_process';
import { getPendingCommits, readAdminUpdateState, startAdminUpdate } from './adminUpdate.js';

function withStatePath(fn: (statePath: string) => void): void {
  const dir = mkdtempSync(join(tmpdir(), 'gah-admin-update-'));
  const statePath = join(dir, 'admin-update-state.json');
  const saved = process.env.GAH_ADMIN_UPDATE_STATE_PATH;
  process.env.GAH_ADMIN_UPDATE_STATE_PATH = statePath;
  try {
    fn(statePath);
  } finally {
    if (saved === undefined) delete process.env.GAH_ADMIN_UPDATE_STATE_PATH;
    else process.env.GAH_ADMIN_UPDATE_STATE_PATH = saved;
    rmSync(dir, { recursive: true, force: true });
  }
}

function fakeChildProcess(pid: number): ChildProcess {
  const child = new EventEmitter() as unknown as {
    pid: number;
    unref: () => void;
    stdout: EventEmitter;
    stderr: EventEmitter;
  };
  child.pid = pid;
  child.unref = () => {};
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  return child as unknown as ChildProcess;
}

test('readAdminUpdateState returns idle when no state file exists', () => {
  withStatePath(() => {
    assert.deepEqual(readAdminUpdateState(), {
      status: 'idle',
      startedAt: null,
      finishedAt: null,
      exitCode: null,
      pid: null,
      output: ''
    });
  });
});

test('readAdminUpdateState reconciles a running state whose pid died into inferred_restart', () => {
  withStatePath((statePath) => {
    const deadPid = 999_999_999; // far past any real pid -- pidAlive() must report false.
    writeFileSync(
      statePath,
      JSON.stringify({
        status: 'running',
        startedAt: '2026-01-01T00:00:00.000Z',
        finishedAt: null,
        exitCode: null,
        pid: deadPid,
        output: 'building...'
      })
    );

    const state = readAdminUpdateState();
    assert.equal(state.status, 'inferred_restart');
    assert.equal(state.pid, deadPid);
    assert.equal(state.output, 'building...');
    assert.ok(state.finishedAt);

    // The reconciliation must persist -- a second read (e.g. from a
    // subsequent poll) must not depend on re-deriving it, and must not flip
    // back if the dead pid happens to get reused by an unrelated process.
    const onDisk = JSON.parse(readFileSync(statePath, 'utf8')) as { status: string };
    assert.equal(onDisk.status, 'inferred_restart');
  });
});

test('readAdminUpdateState leaves a running state alone while its pid is still alive', () => {
  withStatePath((statePath) => {
    writeFileSync(
      statePath,
      JSON.stringify({
        status: 'running',
        startedAt: '2026-01-01T00:00:00.000Z',
        finishedAt: null,
        exitCode: null,
        pid: process.pid,
        output: ''
      })
    );
    const state = readAdminUpdateState();
    assert.equal(state.status, 'running');
    assert.equal(state.finishedAt, null);
  });
});

test('startAdminUpdate launches gah update pinned to this checkout via --repo and records output/exit', () => {
  withStatePath(() => {
    let capturedBin = '';
    let capturedArgs: string[] = [];
    const child = fakeChildProcess(4242);
    const spawnFn = ((bin: string, args: string[]) => {
      capturedBin = bin;
      capturedArgs = args;
      return child;
    }) as unknown as typeof spawn;

    const result = startAdminUpdate({ spawnFn });
    assert.equal(result.started, true);
    assert.equal(result.state.status, 'running');
    assert.equal(result.state.pid, 4242);
    assert.ok(capturedBin);
    assert.deepEqual(capturedArgs, ['update', '--repo', process.cwd(), '--role', 'central', '--restart-server']);

    (child.stdout as unknown as EventEmitter).emit('data', Buffer.from('hello '));
    (child.stderr as unknown as EventEmitter).emit('data', Buffer.from('world'));
    (child as unknown as EventEmitter).emit('close', 0);

    const finalState = readAdminUpdateState();
    assert.equal(finalState.status, 'success');
    assert.equal(finalState.output, 'hello world');
    assert.equal(finalState.exitCode, 0);
  });
});

test('startAdminUpdate records failure on a non-zero exit code', () => {
  withStatePath(() => {
    const child = fakeChildProcess(4243);
    const spawnFn = (() => child) as unknown as typeof spawn;
    startAdminUpdate({ spawnFn });
    (child as unknown as EventEmitter).emit('close', 1);
    assert.equal(readAdminUpdateState().status, 'failed');
    assert.equal(readAdminUpdateState().exitCode, 1);
  });
});

test('startAdminUpdate refuses a second launch while the previous updater pid is still alive', () => {
  withStatePath((statePath) => {
    writeFileSync(
      statePath,
      JSON.stringify({
        status: 'running',
        startedAt: new Date().toISOString(),
        finishedAt: null,
        exitCode: null,
        pid: process.pid,
        output: ''
      })
    );
    let spawnCalled = false;
    const spawnFn = (() => {
      spawnCalled = true;
      return fakeChildProcess(1);
    }) as unknown as typeof spawn;

    const result = startAdminUpdate({ spawnFn });
    assert.equal(result.started, false);
    assert.equal(spawnCalled, false);
  });
});

test('getPendingCommits reports commitsBehind from a stubbed git', () => {
  const gitFn = ((_bin: string, args: string[]) => {
    if (args[0] === 'log' && args[args.length - 1] === 'HEAD') {
      return { status: 0, stdout: 'aaa111|aaa1|Fix bug\n' } as ReturnType<typeof spawnSync>;
    }
    if (args[0] === 'log' && args[args.length - 1] === 'origin/main') {
      return { status: 0, stdout: 'bbb222|bbb2|Add feature\n' } as ReturnType<typeof spawnSync>;
    }
    if (args[0] === 'rev-list') {
      return { status: 0, stdout: '3\n' } as ReturnType<typeof spawnSync>;
    }
    throw new Error(`unexpected git call: ${args.join(' ')}`);
  }) as unknown as typeof spawnSync;

  const pending = getPendingCommits('/repo', { gitFn });
  assert.deepEqual(pending.current, { hash: 'aaa111', short: 'aaa1', subject: 'Fix bug' });
  assert.deepEqual(pending.latest, { hash: 'bbb222', short: 'bbb2', subject: 'Add feature' });
  assert.equal(pending.commitsBehind, 3);
  assert.equal(pending.upToDate, false);
});

test('getPendingCommits reports upToDate when HEAD matches origin/main', () => {
  const gitFn = (() => ({ status: 0, stdout: 'aaa111|aaa1|Fix bug\n' }) as ReturnType<typeof spawnSync>) as unknown as typeof spawnSync;
  const pending = getPendingCommits('/repo', { gitFn });
  assert.equal(pending.upToDate, true);
  assert.equal(pending.commitsBehind, 0);
});
