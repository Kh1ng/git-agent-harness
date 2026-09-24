import assert from 'node:assert/strict';
import { appendFileSync, mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import type { ChatUsage } from '@git-agent-harness/contracts';
import type { ManagerAdapter } from './registry.js';
import {
  chatTitleInput,
  commitMessageInput,
  prSummaryInput,
  readHelperUsage,
  recordHelperUsage,
  runHelperTask,
  sanitizeHelperInput
} from './helperTasks.js';

const usage: ChatUsage = {
  input_tokens: 10,
  output_tokens: 3,
  total_tokens: 13,
  estimated_cost_usd: 0.001,
  duration_seconds: 0.2
};

function fakeAdapter(models: string[], reply: string, seen: { model?: string | null; cwd?: string; modelCwd?: string }): ManagerAdapter {
  return {
    id: 'codex', displayName: 'Codex', implemented: true,
    async listModels(_key, cwd?: string) {
      seen.modelCwd = cwd;
      return { models: models.map(id => ({ id, name: id })), currentModelId: 'coding-model', reasoningEfforts: [], currentReasoningEffortId: null, contextUsage: null };
    },
    async runTurn(_key, input) {
      seen.model = input.model;
      seen.cwd = input.cwd;
      return { reply, model: input.model ?? null, usage };
    },
    async listCommands() { return []; },
    async setModel() {}, async setReasoningEffort() {}, async cancelTurn() {},
    async steerTurn() { return { outcome: 'injected' }; }
  };
}

test('Codex helpers use the advertised Luna model on the active account', async () => {
  const seen: { account?: string | null; model?: string | null; cwd?: string; modelCwd?: string } = {};
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
  assert.ok(seen.cwd);
  assert.equal(seen.modelCwd, seen.cwd);
  assert.notEqual(seen.cwd, process.cwd());
  assert.deepEqual(readdirSync(seen.cwd), []);
  assert.equal(result.text, 'Worker update flow');
  assert.equal(result.backendInstance, 'codex-personal');
  assert.equal(result.requestedModel, null);
  assert.equal(result.effectiveModel, 'gpt-6-luna-2026-09');
  assert.equal(result.actualModel, 'gpt-6-luna-2026-09');
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
  assert.equal(result.backendInstance, 'codex-personal');
  assert.equal(result.requestedModel, 'gpt-6-luna');
});

test('helper inputs are bounded and missing models use deterministic fallback', async () => {
  assert.equal(chatTitleInput('x'.repeat(2_000)).length, 800);
  const commitInput = commitMessageInput([{ path: 'selected.ts', staged: true, unstaged: false, untracked: false }], 'd'.repeat(100_000));
  assert.ok(commitInput.length < 49_000);
  assert.match(commitInput, /selected\.ts \(staged\)/);
  const prInput = prSummaryInput({
    branch: 'fix/issue-42', base: 'main', patch: 'diff',
    commits: [{ hash: 'a', short: 'a', subject: 'Fix #42' }]
  }, [{ number: 42, title: 'Keep worker transport', labels: ['windows', 'networking'] }]);
  assert.match(prInput, /#42 Keep worker transport \[windows, networking\]/);
  const result = await runHelperTask({
    profile: 'gah', sourceBackend: 'codex', sourceBackendInstance: null,
    kind: 'chat_title', input: 'message', fallback: { text: 'message' }
  }, { adapter: async () => fakeAdapter(['gpt-6-codex'], 'unused', {}) });
  assert.equal(result.text, 'message');
  assert.equal(result.fallbackReason, 'missing_model');
});

test('helper input redacts provider tokens, configured secrets, and credential assignments', () => {
  const previous = process.env.GAH_TEST_API_KEY;
  process.env.GAH_TEST_API_KEY = 'private-value-1234';
  try {
    const clean = sanitizeHelperInput([
      'branch: fix/sk-' + 'x'.repeat(24),
      'subject: use github_pat_' + 'y'.repeat(24),
      'note: private-value-1234',
      '+API_KEY=visible-secret'
    ].join('\n'));
    assert.doesNotMatch(clean, /sk-x|github_pat_|private-value-1234|visible-secret/);
    assert.match(clean, /REDACTED/);
    assert.match(clean, /credential line omitted/);
  } finally {
    if (previous === undefined) delete process.env.GAH_TEST_API_KEY; else process.env.GAH_TEST_API_KEY = previous;
  }
});

test('helper telemetry keeps task, account, model, usage, latency, and fallback separate', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-helper-usage-'));
  const previous = process.env.GAH_CHAT_STATE_DIR;
  process.env.GAH_CHAT_STATE_DIR = directory;
  try {
    recordHelperUsage('gah', {
      kind: 'commit_message', text: 'Fix updates', generated: true, backend: 'codex', backendInstance: 'codex-work',
      requestedModel: null, effectiveModel: 'gpt-6-luna', actualModel: 'gpt-6-luna', fallbackReason: null, usage, latencyMs: 220
    });
    appendFileSync(join(directory, 'helper-usage.jsonl'), '{broken json}\n');
    recordHelperUsage('gah', {
      kind: 'chat_title', text: 'Title', generated: false, backend: 'codex', backendInstance: 'codex-work',
      requestedModel: null, effectiveModel: null, actualModel: null, fallbackReason: 'missing_model', usage: null, latencyMs: 5
    });
    const records = readHelperUsage();
    assert.deepEqual(records, [{
      timestamp: records[0].timestamp,
      kind: 'commit_message', profile: 'gah', backend: 'codex', backendInstance: 'codex-work',
      requestedModel: null, effectiveModel: 'gpt-6-luna', actualModel: 'gpt-6-luna',
      inputTokens: 10, outputTokens: 3, totalTokens: 13, latencyMs: 220, fallbackReason: null
    }, {
      timestamp: records[1].timestamp,
      kind: 'chat_title', profile: 'gah', backend: 'codex', backendInstance: 'codex-work',
      requestedModel: null, effectiveModel: null, actualModel: null,
      inputTokens: null, outputTokens: null, totalTokens: null, latencyMs: 5, fallbackReason: 'missing_model'
    }]);
  } finally {
    if (previous === undefined) delete process.env.GAH_CHAT_STATE_DIR; else process.env.GAH_CHAT_STATE_DIR = previous;
    rmSync(directory, { recursive: true, force: true });
  }
});
