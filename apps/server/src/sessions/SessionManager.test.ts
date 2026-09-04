import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ServerMessage } from '@git-agent-harness/contracts';
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
        childProcess: null
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
  const statusChanges: Array<{ id: string; status: string }> = [];
  let dispatchCalls = 0;
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: {
      publish(message) {
        if (message.type === 'session.status') statusChanges.push({ id: message.session.id, status: message.session.status });
      }
    },
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
        childProcess: null
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

  const [first, second] = await Promise.all([
    manager.startSession({ ...options, target: '#1' }),
    manager.startSession({ ...options, target: '#2' })
  ]);
  assert.equal(dispatchCalls, 1);
  assert.equal(first.status, 'running');
  assert.equal(second.status, 'starting');
  assert.deepEqual(statusChanges, [{ id: first.id, status: 'running' }]);

  releases.shift()?.();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(dispatchCalls, 2);
  assert.equal(second.status, 'running');
  assert.deepEqual(statusChanges, [
    { id: first.id, status: 'running' },
    { id: second.id, status: 'running' }
  ]);
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
      childProcess: null
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
        childProcess: null
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

  // Start first session
  const session1 = await manager.startSession({ ...options, target: '#1' });
  assert.equal(dispatchCalls, 1);
  
  // Queue second session (same profile)
  const session2Promise = manager.startSession({ ...options, target: '#2' });
  
  // Stop the first session
  await manager.stopSession(session1.id);
  
  // Give a moment for the second session to be processed
  await new Promise(resolve => setTimeout(resolve, 10));
  
  // The second session should have been cancelled before starting
  const session2 = await session2Promise;
  
  // Only one dispatch should have been called (the first one)
  assert.equal(dispatchCalls, 1);
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
      childProcess: null
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
  const manager = createSessionManager({
    disableCleanupTimer: true,
    providerRegistry: { isProviderAvailable: () => true },
    pushBus: { publish() {} },
    dispatchRunner: () => {
      let resolveDispatch: (result: { exitCode: number, stderr: string }) => void;
      const promise = new Promise<{ exitCode: number, stderr: string }>((resolve) => {
        resolveDispatch = resolve;
      });
      
      return {
        promise,
        cancel: async () => {
          // Even though we cancel, the process might still complete later
          return { cancelled: true };
        },
        childProcess: null,
        // Add a method to simulate late completion
        simulateLateCompletion: () => {
          resolveDispatch!({ exitCode: 0, stderr: '' });
        }
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

  // Stop the session (this should set status to stopping/stopped)
  await manager.stopSession(session.id);
  
  // Now simulate a late completion - this should not overwrite the cancelled state
  const activeDispatch = Array.from((manager as any).activeDispatches.values()).find(
    (d: any) => d.sessionId === session.id
  );
  if (activeDispatch) {
    // This is a bit of a hack to access the internal structure, but for testing purposes
    const cancellableDispatch = activeDispatch.cancellableDispatch;
    if (cancellableDispatch && typeof (cancellableDispatch as any).simulateLateCompletion === 'function') {
      (cancellableDispatch as any).simulateLateCompletion();
    }
  }
  
  // Wait a bit for any late completion to be processed
  await new Promise(resolve => setTimeout(resolve, 50));
  
  // Check that the session is still stopped, not overwritten by the late completion
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
      childProcess: null
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
  const timeout = new Promise((_, reject) => setTimeout(() => reject(new Error('Test timeout')), 35000));
  const stopPromise = Promise.race([
    manager.stopSession(session.id),
    timeout
  ]);
  
  const stoppedSession = await stopPromise;
  
  // Should have timed out and marked as error
  assert.equal(stoppedSession.status, 'error');
  assert.equal(stoppedSession.error, 'Session cancellation timed out');
});
