import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { usageRollup } from './usageRollup.js';
import { setChatSessionStoreOptions } from './chatSessions.js';
import { appendEvents } from './sessionLog.js';

const NOW = Date.parse('2026-08-30T12:00:00Z');
const DAY = 86_400_000;

function fixture(): string {
  const stateDir = join(tmpdir(), `gah-usage-rollup-${Date.now()}-${Math.random().toString(36).slice(2)}`);
  mkdirSync(stateDir, { recursive: true });
  setChatSessionStoreOptions({ stateDir });
  return stateDir;
}

function sessionLog(stateDir: string, profile: string, sessionId: string): void {
  const dir = join(stateDir, `project-${encodeURIComponent(profile)}`, `session-${sessionId}`);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'session.jsonl'), '');
}

test('rollup aggregates usage by backend, model, and UTC day', () => {
  const stateDir = fixture();
  try {
    sessionLog(stateDir, 'repo', 'a');
    appendEvents('repo', [
      { type: 'assistant/message', seq: 1, turn: 1, text: 'one', backend: 'codex', model: 'gpt-5.3', usage: { input_tokens: 100, output_tokens: 50, total_tokens: 150, estimated_cost_usd: 0.01, duration_seconds: 1 }, timestamp: NOW },
      { type: 'assistant/message', seq: 2, turn: 2, text: 'two', backend: 'codex', model: 'gpt-5.3', usage: { input_tokens: 30, output_tokens: 20, total_tokens: 50, estimated_cost_usd: 0.005, duration_seconds: 1 }, timestamp: NOW - 1000 },
      // Same backend, different model: separate row.
      { type: 'assistant/message', seq: 3, turn: 3, text: 'three', backend: 'codex', model: 'gpt-5.3-spark', usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15, estimated_cost_usd: 0, duration_seconds: 1 }, timestamp: NOW },
      // Different backend, same day.
      { type: 'assistant/message', seq: 4, turn: 4, text: 'four', backend: 'claude', model: null, usage: { input_tokens: 7, output_tokens: 3, total_tokens: 10, estimated_cost_usd: 0, duration_seconds: 1 }, timestamp: NOW },
      // Yesterday: separate day row, still inside a 7d window.
      { type: 'assistant/message', seq: 5, turn: 5, text: 'five', backend: 'codex', model: 'gpt-5.3', usage: { input_tokens: 500, output_tokens: 500, total_tokens: 1000, estimated_cost_usd: 0.1, duration_seconds: 1 }, timestamp: NOW - DAY }
    ], { stateDir, sessionId: 'a' });

    const rollup = usageRollup('repo', 7, { stateDir, now: () => NOW });
    assert.equal(rollup.rows.length, 4);
    assert.equal(rollup.unattributed_turns, 0);

    const codexToday = rollup.rows.find((row) => row.backend === 'codex' && row.model === 'gpt-5.3' && row.day === '2026-08-30');
    assert.ok(codexToday);
    assert.equal(codexToday.turns, 2);
    assert.equal(codexToday.total_tokens, 200);
    assert.ok(Math.abs(codexToday.estimated_cost_usd - 0.015) < 1e-9);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(stateDir, { recursive: true, force: true });
  }
});

test('rollup counts usage-less turns as unattributed and honors the window', () => {
  const stateDir = fixture();
  try {
    sessionLog(stateDir, 'repo', 'b');
    appendEvents('repo', [
      { type: 'assistant/message', seq: 1, turn: 1, text: 'no usage', backend: 'agy', model: null, usage: null, timestamp: NOW - 1000 },
      { type: 'assistant/message', seq: 2, turn: 2, text: 'outside window', backend: 'codex', model: null, usage: { input_tokens: 999, output_tokens: 999, total_tokens: 1998, estimated_cost_usd: 1, duration_seconds: 1 }, timestamp: NOW - 8 * DAY }
    ], { stateDir, sessionId: 'b' });

    const week = usageRollup('repo', 7, { stateDir, now: () => NOW });
    assert.equal(week.rows.length, 0, 'the 8-day-old turn is outside the window');
    assert.equal(week.unattributed_turns, 1);

    const month = usageRollup('repo', 30, { stateDir, now: () => NOW });
    assert.equal(month.rows.length, 1);
    assert.equal(month.unattributed_turns, 1);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(stateDir, { recursive: true, force: true });
  }
});
