/**
 * Unit tests for Dispatch Router
 */

import assert from 'assert';
import { chooseHost } from './dispatchRouter.js';
import { getSessionManager } from '../sessions/SessionManager.js';
import type { Session } from '@git-agent-harness/contracts';

console.log('Running dispatchRouter unit tests...');

// Mock the getSessionManager().getActiveSessions()
const sessionManager = getSessionManager();

// Helper to clear sessions
// Since sessions is private, we can cast sessionManager to any to modify it
const sessionsMap = (sessionManager as any).sessions;

function clearSessions() {
  sessionsMap.clear();
}

function addMockSession(id: string, status: 'running' | 'stopped', hostId?: string) {
  sessionsMap.set(id, {
    id,
    providerKind: 'codex',
    instanceId: 'codex_instance',
    status,
    repo: 'test-repo',
    mode: 'dispatch',
    hostId
  } as Session);
}

// 1. Test least_loaded strategy
clearSessions();
addMockSession('s1', 'running', 'local');
addMockSession('s2', 'running', 'host1');
addMockSession('s3', 'running', 'host1');
addMockSession('s4', 'stopped', 'host1'); // stopped session should not count towards load

// Candidates
const candidates = ['local', 'host1', 'host2'];

// host2 has 0 active sessions
// local has 1 active session
// host1 has 2 active sessions (s2 and s3 are running, s4 is stopped)
let chosen = chooseHost(candidates, 'least_loaded');
assert.strictEqual(chosen, 'host2', 'least_loaded should choose host2 (0 active sessions)');

// Now start one on host2
addMockSession('s5', 'running', 'host2');
// host2 now has 1 active session
// local has 1 active session
// host1 has 2 active sessions
// least_loaded should choose local or host2 (since both have 1, it picks first matching candidate which is local)
chosen = chooseHost(candidates, 'least_loaded');
assert.strictEqual(chosen, 'local', 'least_loaded should choose local (1 active session)');

console.log('✅ least_loaded tests passed');

// 2. Test round_robin strategy
clearSessions();
// Resetting or checking round_robin cycling
const rrChosen: string[] = [];
for (let i = 0; i < 5; i++) {
  rrChosen.push(chooseHost(candidates, 'round_robin'));
}
assert.deepStrictEqual(rrChosen, ['local', 'host1', 'host2', 'local', 'host1'], 'round_robin should cycle through candidates');
console.log('✅ round_robin tests passed');

// 3. Test pinned strategy
clearSessions();
chosen = chooseHost(candidates, 'pinned', 'host1');
assert.strictEqual(chosen, 'host1', 'pinned should choose host1');

chosen = chooseHost(candidates, 'pinned', 'nonexistent');
assert.strictEqual(chosen, 'local', 'pinned should fallback to local for invalid host');

chosen = chooseHost(candidates, 'pinned');
assert.strictEqual(chosen, 'local', 'pinned should fallback to local when no pinnedHostId is provided');
console.log('✅ pinned tests passed');

console.log('All dispatchRouter tests passed successfully!');
process.exit(0);
