import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import type { ChatUsage } from '@git-agent-harness/contracts';
import type { ManagerAdapter } from './registry.js';
import {
  chatTitleInput,
  commitMessageInput,
  readHelperUsage,
  recordHelperUsage,
  runHelperTask
} from './helperTasks.js';

const usage: ChatUsage = {
  input_tokens: 10,
  output_tokens: 3,
  total_tokens: 13,
  estimated_cost_usd: 0.001,
  duration_seconds: 0.2
};

function fakeAdapter(models: string[], reply: string, seen: { model?: string | null }): ManagerAdapter {
  return {
    id: 'codex', displayName: 'Codex', implemented: true,
    async listModels() {
      return { models: models.map(id => ({ id, name: id })), currentModelId: 'coding-model', reasoningEfforts: [], currentReasoningEffortId: null, contextUsage: null };
    },
    async runTurn(_key, input) {
      seen.model = input.model;
      return { reply, model: input.model ?? null, usage };
    },
    async listCommands() { return []; },
    async setModel() {}, async setReasoningEffort() {}, async cancelTurn() {},
    async steerTurn() { return { outcome: 'injected' }; }
  };
}

test('Codex helpers use the advertised Luna model on the active account', async () => {
  const seen: { account?: string | null; model?: string | null } = {};
  const result = await runHelperTask({
    profile: 'gah', sourceBackend: 'codex', sourceBackendInstance: 'codex-personal',
    kind: 'chat_title', input: chatTitleInput('Fix the worker update flow'),
    fallback: { text: 'Fix the worker update flow' }
  }, {
    adapter: async (_profile, _backend, instance) => {
      seen.account = instance;
      return fakeAdapter(['gpt-6-codex', 'gpt-6-luna-2026-09'], 'Worker update flow', seen);
    }
  });
  assert.equal(seen.account, 'codex-personal');
  assert.equal(seen.model, 'gpt-6-luna-2026-09');
  assert.equal(result.text, 'Worker update flow');
  assert.equal(result.backendInstance, 'codex-personal');
  assert.equal(result.usage?.total_tokens, 13);
});

test('a configured account failure never falls through to another account', async () => {
  const accounts: Array<string | null> = [];
  const result = await runHelperTask({
    profile: 'gah', sourceBackend: 'codex', sourceBackendInstance: 'codex-work',
    kind: 'commit_message', input: 'diff', fallback: { text: '' }
  }, {
    preference: {
      profile: 'gah', sourceBackend: 'codex', sourceBackendInstance: 'codex-work', enabled: true,
      backend: 'codex', backendInstance: 'codex-personal', model: 'gpt-6-luna'
    },
    adapter: async (_profile, _backend, instance) => {
      accounts.push(instance);
      throw new Error('Login required');
    }
  });
  assert.deepEqual(accounts, ['codex-personal']);
  assert.equal(result.generated, false);
  assert.equal(result.fallbackReason, 'missing_auth');
});

test('helper inputs are bounded and missing models use deterministic fallback', async () => {
  assert.equal(chatTitleInput('x'.repeat(2_000)).length, 800);
  assert.ok(commitMessageInput(['selected.ts'], 'd'.repeat(100_000)).length < 49_000);
  const result = await runHelperTask({
    profile: 'gah', sourceBackend: 'codex', sourceBackendInstance: null,
    kind: 'chat_title', input: 'message', fallback: { text: 'message' }
  }, { adapter: async () => fakeAdapter(['gpt-6-codex'], 'unused', {}) });
  assert.equal(result.text, 'message');
  assert.equal(result.fallbackReason, 'missing_model');
});

test('helper telemetry keeps task, account, model, usage, latency, and fallback separate', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-helper-usage-'));
  const previous = process.env.GAH_CHAT_STATE_DIR;
  process.env.GAH_CHAT_STATE_DIR = directory;
  try {
    recordHelperUsage('gah', {
      kind: 'commit_message', text: 'Fix updates', generated: true, backend: 'codex', backendInstance: 'codex-work',
      model: 'gpt-6-luna', fallbackReason: null, usage, latencyMs: 220
    });
    const records = readHelperUsage();
    assert.deepEqual(records, [{
      timestamp: records[0].timestamp,
      kind: 'commit_message', profile: 'gah', backend: 'codex', backendInstance: 'codex-work', model: 'gpt-6-luna',
      inputTokens: 10, outputTokens: 3, totalTokens: 13, latencyMs: 220, fallbackReason: null
    }]);
  } finally {
    if (previous === undefined) delete process.env.GAH_CHAT_STATE_DIR; else process.env.GAH_CHAT_STATE_DIR = previous;
    rmSync(directory, { recursive: true, force: true });
  }
});
