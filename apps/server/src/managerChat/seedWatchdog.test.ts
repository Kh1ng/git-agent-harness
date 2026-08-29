import assert from 'node:assert/strict';
import { test } from 'node:test';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChatSessionSummary } from '@git-agent-harness/contracts';
import { appendEvents, readLog } from './sessionLog.js';
import { setChatSessionStoreOptions } from './chatSessions.js';
import { runSeedWatchdogScan, SEED_WATCHDOG_MARKER } from './seedWatchdog.js';

const NOW = 1_800_000_000_000;
const DEADLINE = 2 * 60_000;

interface Fixture {
  stateDir: string;
  seedSession(profile: string, sessionId: string, createdAt: number): void;
  addTurnEvents(profile: string, sessionId: string, extra: 'none' | 'dispatched'): void;
}

function makeFixture(): Fixture {
  const stateDir = join(tmpdir(), `gah-seed-watchdog-${Date.now()}-${Math.random().toString(36).slice(2)}`);
  mkdirSync(stateDir, { recursive: true });
  setChatSessionStoreOptions({ stateDir });

  return {
    stateDir,
    seedSession(profile, sessionId, createdAt) {
      const profileDir = join(stateDir, `project-${encodeURIComponent(profile)}`);
      mkdirSync(profileDir, { recursive: true });
      const session: ChatSessionSummary = {
        id: sessionId,
        profile,
        worktreePath: null,
        branch: `gah/chat/x-${sessionId}`,
        backend: 'codex',
        model: null,
        reasoningEffort: null,
        title: null,
        createdAt,
        lastActiveAt: createdAt,
        archivedAt: null,
        outcome: 'live',
        settledAt: null,
        settledReason: null
      };
      const indexPath = join(profileDir, 'sessions.json');
      const index = existsSync(indexPath)
        ? JSON.parse(readFileSync(indexPath, 'utf8')) as { sessions: ChatSessionSummary[] }
        : { sessions: [] as ChatSessionSummary[] };
      index.sessions = index.sessions.filter((existing) => existing.id !== sessionId);
      index.sessions.push(session);
      writeFileSync(indexPath, JSON.stringify(index));
      appendEvents(profile, [
        { type: 'turn/start', seq: 1, turn: 1, timestamp: createdAt },
        { type: 'user/message', seq: 2, turn: 1, text: '#1042 the ticket body', source: 'prompt', timestamp: createdAt },
        { type: 'turn/end', seq: 3, turn: 1, reason: { kind: 'complete' }, timestamp: createdAt }
      ], { stateDir, sessionId });
    },
    addTurnEvents(profile, sessionId, extra) {
      if (extra === 'none') return;
      const logOpts = { stateDir, sessionId };
      const events = readLog(profile, logOpts);
      const nextSeq = events.reduce((highest, event) => Math.max(highest, event.seq), 0) + 1;
      appendEvents(profile, [
        { type: 'turn/start', seq: nextSeq, turn: 2, timestamp: NOW },
        { type: 'user/message', seq: nextSeq + 1, turn: 2, text: 'Implement #1042 now', source: 'prompt', timestamp: NOW },
        { type: 'assistant/chunk', seq: nextSeq + 2, turn: 2, text: 'On it', timestamp: NOW }
      ], logOpts);
    }
  };
}

test('a seeded session past the deadline is flagged exactly once', () => {
  const fixture = makeFixture();
  try {
    fixture.seedSession('repo', 'stalled', NOW - DEADLINE - 1000);
    fixture.seedSession('repo', 'fresh', NOW - 1000);

    const flagged = runSeedWatchdogScan({ now: () => NOW, deadlineMs: DEADLINE });
    assert.deepEqual(flagged, ['repo/stalled']);

    const events = readLog('repo', { stateDir: fixture.stateDir, sessionId: 'stalled' });
    const marker = events.find((event) => event.type === 'harness/error' && event.text.startsWith(SEED_WATCHDOG_MARKER));
    assert.ok(marker, 'the durable harness/error marker exists');

    // Rescan: already flagged, and the fresh session is still inside its window.
    assert.deepEqual(runSeedWatchdogScan({ now: () => NOW, deadlineMs: DEADLINE }), []);
    const eventsAfter = readLog('repo', { stateDir: fixture.stateDir, sessionId: 'stalled' });
    assert.equal(eventsAfter.filter((event) => event.type === 'harness/error' && event.text.startsWith(SEED_WATCHDOG_MARKER)).length, 1);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(fixture.stateDir, { recursive: true, force: true });
  }
});

test('a session that received its dispatch turn is never flagged', () => {
  const fixture = makeFixture();
  try {
    fixture.seedSession('repo', 'dispatched', NOW - DEADLINE - 1000);
    fixture.addTurnEvents('repo', 'dispatched', 'dispatched');

    assert.deepEqual(runSeedWatchdogScan({ now: () => NOW, deadlineMs: DEADLINE }), []);
    const events = readLog('repo', { stateDir: fixture.stateDir, sessionId: 'dispatched' });
    assert.equal(events.some((event) => event.type === 'harness/error' && event.text.startsWith(SEED_WATCHDOG_MARKER)), false);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(fixture.stateDir, { recursive: true, force: true });
  }
});

test('archived and settled sessions are ignored even when never dispatched', () => {
  const fixture = makeFixture();
  try {
    fixture.seedSession('repo', 'archived', NOW - DEADLINE - 1000);
    const profileDir = join(fixture.stateDir, 'project-repo');
    const indexPath = join(profileDir, 'sessions.json');
    const index = JSON.parse(readFileSync(indexPath, 'utf8'));
    index.sessions[0].archivedAt = NOW;
    index.sessions[0].outcome = 'archived';
    writeFileSync(indexPath, JSON.stringify(index));

    assert.deepEqual(runSeedWatchdogScan({ now: () => NOW, deadlineMs: DEADLINE }), []);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(fixture.stateDir, { recursive: true, force: true });
  }
});
