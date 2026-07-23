import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ServerMessage } from '@git-agent-harness/contracts';
import { createSessionManager } from './SessionManager.js';

test('session.start request ids are idempotent and emit one start event', async () => {
  const published: ServerMessage[] = [];
  let dispatchCalls = 0;
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
    dispatchRunner: async () => {
      dispatchCalls += 1;
      return await new Promise(() => {
        // Keep the dispatch active so the second request simulates a retry
        // against an in-flight run rather than a completed one.
      });
    }
  });

  const session = await manager.startSession({
    requestId: 'dispatch-req-1',
    profile: 'gah',
    providerKind: 'codex',
    instanceId: 'codex-0',
    repo: 'owner/repo',
    mode: 'improve'
  });

  const retry = await manager.startSession({
    requestId: 'dispatch-req-1',
    profile: 'gah',
    providerKind: 'codex',
    instanceId: 'codex-0',
    repo: 'owner/repo',
    mode: 'improve'
  });

  assert.equal(retry.id, session.id);
  assert.equal(dispatchCalls, 1);
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
