import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { appendEvents, chatLogPath, foldSession, loadLog } from './sessionLog.js';
import type { ChatSessionEvent } from '@git-agent-harness/contracts';

function tempStateDir(): string {
  return mkdtempSync(join(tmpdir(), 'gah-chat-log-'));
}

function turnStart(turn: number, seq: number): ChatSessionEvent {
  return { type: 'turn/start', seq, turn, timestamp: 1000 + turn };
}

function userMsg(turn: number, seq: number, text: string): ChatSessionEvent {
  return { type: 'user/message', seq, turn, text, source: 'prompt', timestamp: 2000 + turn };
}

function assistantMsg(turn: number, seq: number, text: string, backend = 'hermes'): ChatSessionEvent {
  return {
    type: 'assistant/message',
    seq,
    turn,
    text,
    backend,
    model: 'model-x',
    usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15, estimated_cost_usd: 0.01, duration_seconds: 1 },
    timestamp: 3000 + turn
  };
}

function turnEnd(turn: number, seq: number, kind: 'complete' | 'interrupted' = 'complete'): ChatSessionEvent {
  return { type: 'turn/end', seq, turn, reason: { kind }, timestamp: 4000 + turn };
}

test('append + fold produces the derived transcript with attribution', () => {
  const dir = tempStateDir();
  try {
    appendEvents('gah', [
      turnStart(1, 1),
      userMsg(1, 2, 'hello'),
      assistantMsg(1, 3, 'hi there'),
      turnEnd(1, 4)
    ], { stateDir: dir });

    const view = foldSession('gah', { stateDir: dir });
    assert.equal(view.cursor, 4);
    assert.equal(view.turns.length, 2);
    assert.deepEqual(view.turns[0], { role: 'user', text: 'hello', timestamp: 2001 });
    assert.equal(view.turns[1].role, 'assistant');
    assert.equal(view.turns[1].text, 'hi there');
    assert.equal(view.turns[1].backend, 'hermes');
    assert.equal(view.turns[1].model, 'model-x');
    assert.equal(view.turns[1].usage?.total_tokens, 15);
    assert.equal(view.streaming, null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('an interrupted turn is repaired with a synthetic turn/end, not truncated', () => {
  const dir = tempStateDir();
  try {
    // Crash mid-turn: turn/start + user message + partial chunk, no turn/end.
    appendEvents('gah', [
      turnStart(1, 1),
      userMsg(1, 2, 'question'),
      { type: 'assistant/chunk', seq: 3, turn: 1, text: 'partial reply', timestamp: 3000 }
    ], { stateDir: dir });

    const events = loadLog('gah', { stateDir: dir });
    assert.equal(events.length, 4);
    const last = events[events.length - 1];
    assert.equal(last.type, 'turn/end');
    assert.equal((last as { reason: { kind: string } }).reason.kind, 'interrupted');

    // The fold preserves the partial reply as an assistant message (repair).
    const view = foldSession('gah', { stateDir: dir });
    const assistantTurn = view.turns.find((t) => t.role === 'assistant');
    assert.ok(assistantTurn, 'partial assistant text must survive the crash repair');
    assert.equal(assistantTurn.text, 'partial reply');
    assert.equal(view.streaming, null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a completed turn is not rewritten and no interrupted marker appears', () => {
  const dir = tempStateDir();
  try {
    appendEvents('gah', [
      turnStart(1, 1),
      userMsg(1, 2, 'q'),
      assistantMsg(1, 3, 'a'),
      turnEnd(1, 4)
    ], { stateDir: dir });

    // Reading twice must not mutate the file (idempotent repair).
    const first = loadLog('gah', { stateDir: dir });
    const second = loadLog('gah', { stateDir: dir });
    assert.equal(first.length, second.length);
    assert.equal(first.length, 4);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('logs are profile-scoped', () => {
  const dir = tempStateDir();
  try {
    appendEvents('gah', [turnStart(1, 1), userMsg(1, 2, 'for gah'), turnEnd(1, 3)], { stateDir: dir });
    assert.ok(chatLogPath('gah', { stateDir: dir }).endsWith('gah.jsonl'));
    const sportsball = foldSession('sportsball', { stateDir: dir });
    assert.equal(sportsball.turns.length, 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('malformed lines are skipped, not fatal', () => {
  const dir = tempStateDir();
  try {
    const path = chatLogPath('gah', { stateDir: dir });
    mkdirSync(dir, { recursive: true });
    writeFileSync(path, '{not json}\n' + JSON.stringify(turnStart(1, 1)) + '\n', 'utf8');

    const view = foldSession('gah', { stateDir: dir });
    // The malformed line is skipped; the valid turn/start is repaired into a
    // complete-but-interrupted turn (synthetic turn/end bumps cursor to 2).
    assert.equal(view.cursor, 2);
    assert.equal(view.turns.length, 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
