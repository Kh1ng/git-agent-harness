import assert from 'node:assert/strict';
import { ChildProcess } from 'node:child_process';
import test from 'node:test';
import { startQuotaRefreshScheduler } from './quotaRefreshScheduler.js';

test('startup refresh is single-flight, bounded, and shutdown cancels future refreshes', async t => {
  t.mock.timers.enable({ apis: ['setInterval', 'setTimeout'] });
  const children: ChildProcess[] = [];
  const killed: ChildProcess[] = [];
  const stop = startQuotaRefreshScheduler({
    intervalMs: 5, timeoutMs: 20,
    spawn: () => {
      const child = new ChildProcess();
      children.push(child);
      return child;
    },
    terminate: async child => { killed.push(child); child.emit('close', null, 'SIGKILL'); }
  });
  try {
    assert.equal(children.length, 1, 'refresh starts immediately');
    t.mock.timers.tick(15);
    assert.equal(children.length, 1, 'ticks cannot overlap a running refresh');
    t.mock.timers.tick(5);
    assert.ok(killed.includes(children[0]), 'hung refresh is terminated');
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    t.mock.timers.tick(5);
    assert.equal(children.length, 2, 'refresh resumes after the old process finishes');
    await stop();
    const count = children.length;
    t.mock.timers.tick(50);
    assert.equal(children.length, count, 'shutdown stops future refreshes');
    assert.ok(children.every(child => killed.includes(child)), 'shutdown terminates the current refresh');
  } finally { await stop(); }
});
