import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ServerMessage, Session } from '@git-agent-harness/contracts';
import { createSessionManager } from './SessionManager.js';

test('session.start request ids are idempotent and emit one start event', async () => {
  const published: ServerMessage[] = [];
  let dispatchCalls = 0;
  let dispatchedMr: string | undefined;
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: {
      isProviderAvailable: () => true
    },
    pushBus: {
      publish(message) {
        published.push(message);
      }
    },
    dispatchRunner: (options) => {
      dispatchCalls += 1;
      dispatchedMr = options.mr;
      return {
        promise: new Promise(() => {
          // Keep the dispatch active so the second request simulates a retry
          // against an in-flight run rather than a completed one.
        }),
        cancel: async () => ({ cancelled: true }),
      };
    }
  });

  const session = await manager.startSession({
    requestId: 'dispatch-req-1',
    profile: 'gah',
    providerKind: 'codex',
    instanceId: 'codex-0',
    repo: 'owner/repo',
    mr: '1100',
    mode: 'improve'
  });

  const retry = await manager.startSession({
    requestId: 'dispatch-req-1',
    profile: 'gah',
    providerKind: 'codex',
    instanceId: 'codex-0',
    repo: 'owner/repo',
    mr: '1100',
    mode: 'improve'
  });

  assert.equal(retry.id, session.id);
  assert.equal(dispatchCalls, 1);
  assert.equal(dispatchedMr, '1100');
  assert.equal(manager.getAllSessions().length, 1);
  assert.equal(manager.getActiveSessions().length, 1);
  assert.equal(
    published.filter((message) => message.type === 'session.started').length,
    1
  );

  await manager.stopSession(session.id);

  assert.equal(manager.getActiveSessions().length, 0);
  assert.equal(
    published.filter((message) => message.type === 'session.stopped').length,
    1
  );
});

test('same-profile sessions queue instead of racing the CLI profile lock', async () => {
  const releases: Array<() => void> = [];
  let dispatchCalls = 0;
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => {
      dispatchCalls += 1;
      let resolvePromise: (value: { exitCode: number; stdout: string; stderr: string }) => void;
      const promise = new Promise<{ exitCode: number; stdout: string; stderr: string }>((resolve) => {
        resolvePromise = resolve;
      });
      
      // Add this resolve function to the releases array for the test
      releases.push(() => resolvePromise!({ exitCode: 0, stdout: '', stderr: '' }));
      
      return {
        promise,
        cancel: async () => {
          resolvePromise!({ exitCode: -1, stdout: '', stderr: 'cancelled' });
          return { cancelled: true };
        },
      };
    }
  });
  const options = {
    profile: 'gah',
    providerKind: 'github' as const,
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  };

  await Promise.all([
    manager.startSession({ ...options, target: '#1' }),
    manager.startSession({ ...options, target: '#2' })
  ]);
  assert.equal(dispatchCalls, 1);

  releases.shift()?.();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(dispatchCalls, 2);
  releases.shift()?.();
});

test('stopSession terminates running dispatch and awaits completion', async () => {
  let cancelCalled = false;
  let cancelResolve: () => void;
  const cancelPromise = new Promise<void>((resolve) => { cancelResolve = resolve; });
  
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => ({
      promise: new Promise(() => {}), // Never completes
      cancel: async () => {
        cancelCalled = true;
        cancelResolve();
        return { cancelled: true };
      },
    })
  });

  const session = await manager.startSession({
    profile: 'gah',
    providerKind: 'github',
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  });

  assert.equal(session.status, 'running');
  assert.equal(cancelCalled, false);

  // Stop the session - should call cancel and await completion
  const stopPromise = manager.stopSession(session.id);
  
  // Wait for cancel to be called
  await cancelPromise;
  
  const stoppedSession = await stopPromise;
  
  assert.equal(cancelCalled, true);
  assert.equal(stoppedSession.status, 'stopped');
});

test('stopSession prevents queued same-profile session from starting', async () => {
  let dispatchCalls = 0;
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => {
      dispatchCalls += 1;
      return {
        promise: new Promise(() => {}), // Never completes
        cancel: async () => ({ cancelled: true }),
      };
    }
  });

  const options = {
    profile: 'gah',
    providerKind: 'github' as const,
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  };

  // Start first session -- occupies the profile slot indefinitely, since its
  // dispatch never resolves.
  const session1 = await manager.startSession({ ...options, target: '#1' });
  assert.equal(dispatchCalls, 1);

  // Queue second session behind the first (same profile). startSession
  // resolves immediately with a session even though the underlying dispatch
  // is still waiting for session1's slot.
  const session2 = await manager.startSession({ ...options, target: '#2' });
  assert.equal(dispatchCalls, 1);

  // Stop the QUEUED session itself -- not the running one. This is the
  // scenario AC2 actually describes: cancelling a session that hasn't
  // started must prevent its own dispatchRunner call.
  await manager.stopSession(session2.id);

  // Now let session1 vacate the profile slot so the queue can advance to
  // where session2 would have started.
  await manager.stopSession(session1.id);

  // Give the queue a moment to process the (cancelled) second slot.
  await new Promise(resolve => setTimeout(resolve, 10));

  // Only the first dispatch should ever have been called -- session2 was
  // cancelled before its turn came up.
  assert.equal(dispatchCalls, 1);
});

test('stopping a running session does not cancel an unrelated queued session on the same profile', async () => {
  let dispatchCalls = 0;
  const releases: Array<(result: { exitCode: number; stderr: string }) => void> = [];
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => {
      dispatchCalls += 1;
      let resolvePromise: (result: { exitCode: number; stderr: string }) => void;
      const promise = new Promise<{ exitCode: number; stderr: string }>((resolve) => {
        resolvePromise = resolve;
      });
      releases.push((result) => resolvePromise(result));
      return {
        promise,
        cancel: async () => {
          resolvePromise({ exitCode: -1, stderr: 'cancelled' });
          return { cancelled: true };
        },
      };
    }
  });

  const options = {
    profile: 'gah',
    providerKind: 'github' as const,
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  };

  const session1 = await manager.startSession({ ...options, target: '#1' });
  const session2 = await manager.startSession({ ...options, target: '#2' }); // queued behind session1
  assert.equal(dispatchCalls, 1);

  // Stop the RUNNING session (session1). This must not poison the profile
  // for session2, which never asked to be cancelled -- it should still get
  // its turn to dispatch.
  await manager.stopSession(session1.id);

  // Give the queue a moment to advance now that session1's slot released.
  await new Promise(resolve => setTimeout(resolve, 10));

  assert.equal(dispatchCalls, 2);
  assert.notEqual(manager.getSession(session2.id)?.status, 'error');

  // Drain both dispatch promises so nothing is left dangling.
  releases.shift()?.({ exitCode: 0, stderr: '' });
  releases.shift()?.({ exitCode: 0, stderr: '' });
});

test('stopSession does not publish stopped until cancellation completes', async () => {
  const published: ServerMessage[] = [];
  let cancelResolve: () => void;
  const cancelPromise = new Promise<void>((resolve) => { cancelResolve = resolve; });
  
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: {
      publish(message) {
        published.push(message);
      }
    },
    dispatchRunner: () => ({
      promise: new Promise(() => {}),
      cancel: async () => {
        // Delay the cancellation to test timing
        await new Promise(resolve => setTimeout(resolve, 50));
        cancelResolve();
        return { cancelled: true };
      },
    })
  });

  const session = await manager.startSession({
    profile: 'gah',
    providerKind: 'github',
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  });

  // Start stopping the session
  const stopPromise = manager.stopSession(session.id);
  
  // Check that we have the stopping status message but not the stopped message yet
  const statusMessagesBefore = published.filter(m => m.type === 'session.status');
  const stoppedMessagesBefore = published.filter(m => m.type === 'session.stopped');
  
  assert.equal(statusMessagesBefore.length >= 1, true);
  assert.equal(stoppedMessagesBefore.length, 0);
  
  // Wait for cancellation to complete
  await cancelPromise;
  
  // Now the stopped message should be published
  await stopPromise;
  
  const stoppedMessagesAfter = published.filter(m => m.type === 'session.stopped');
  assert.equal(stoppedMessagesAfter.length, 1);
});

test('late process completion cannot overwrite cancelled terminal state', async () => {
  // Captured directly from the dispatchRunner closure that backs this
  // session -- not recovered by reaching into manager internals after
  // stopSession() has already removed the activeDispatches entry, which
  // would make the "late completion" branch below unreachable.
  let simulateLateCompletion: (() => void) | undefined;

  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => {
      let resolveDispatch: (result: { exitCode: number; stderr: string }) => void;
      const promise = new Promise<{ exitCode: number; stderr: string }>((resolve) => {
        resolveDispatch = resolve;
      });
      simulateLateCompletion = () => resolveDispatch({ exitCode: 0, stderr: '' });

      return {
        promise,
        // cancel() reports success without settling `promise` -- modeling a
        // process whose cancellation was confirmed but whose result
        // callback fires on a separate, later tick.
        cancel: async () => ({ cancelled: true })
      };
    }
  });

  const session = await manager.startSession({
    profile: 'gah',
    providerKind: 'github',
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  });

  await manager.stopSession(session.id);
  assert.equal(manager.getSession(session.id)?.status, 'stopped');

  // Now fire the late completion -- this must hit handleDispatchComplete's
  // terminal-status guard and not overwrite the already-stopped session.
  assert.equal(typeof simulateLateCompletion, 'function');
  simulateLateCompletion?.();

  await new Promise(resolve => setTimeout(resolve, 10));

  const finalSession = manager.getSession(session.id);
  assert.equal(finalSession?.status, 'stopped');
});

test('stopSession times out if cancellation takes too long', async () => {
  const published: ServerMessage[] = [];
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: {
      publish(message) {
        published.push(message);
      }
    },
    dispatchRunner: () => ({
      promise: new Promise(() => {}),
      cancel: async () => {
        // This will never resolve, simulating a hung process
        await new Promise(() => {});
        return { cancelled: true };
      },
    })
  });

  const session = await manager.startSession({
    profile: 'gah',
    providerKind: 'github',
    instanceId: 'github-0',
    repo: 'owner/repo',
    mode: 'fix'
  });

  // This should timeout and still complete
  const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error('Test timeout')), 35000));
  const stopPromise: Promise<Session> = Promise.race([
    manager.stopSession(session.id),
    timeout
  ]);
  
  const stoppedSession = await stopPromise;
  
  // Should have timed out and marked as error
  assert.equal(stoppedSession.status, 'error');
  assert.equal(stoppedSession.error, 'Session cancellation timed out');
});
