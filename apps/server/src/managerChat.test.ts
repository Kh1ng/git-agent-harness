import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { compactionSummary, isCompactionCommand, isUsageLimitError } from './managerChat/acpAdapter.js';
import { normalizeRemoteUrl } from './managerChat/memoryGatewayClient.js';
import {
  modelOverrideForProfile,
  reasoningEffortOverrideForProfile,
  setModelOverrideForProfile,
  setReasoningEffortOverrideForProfile
} from './managerChat/settingsStore.js';
import { historyDelta, readModelConfig, readReasoningConfig, resumePrompt, toChatUsage, toContextUsage } from './managerChat/acpAdapter.js';
import { handoffAttempt } from './managerChat/ManagerChatManager.js';

test('isCompactionCommand recognizes known compact/clear synonyms across backends', () => {
  assert.equal(isCompactionCommand('/compact'), true);
  assert.equal(isCompactionCommand('/compress'), true);
  assert.equal(isCompactionCommand('/clear'), true);
  assert.equal(isCompactionCommand('/reset'), true);
  assert.equal(isCompactionCommand('/RESET'), true);
  assert.equal(isCompactionCommand('  /compact  '), true);
});

test('isCompactionCommand ignores unrelated slash commands and plain text', () => {
  assert.equal(isCompactionCommand('/help'), false);
  assert.equal(isCompactionCommand('/model gpt'), false);
  assert.equal(isCompactionCommand('please clear this up'), false);
  assert.equal(isCompactionCommand(''), false);
  assert.equal(isCompactionCommand('/'), false);
});

test('compaction always leaves a durable replacement summary', () => {
  assert.equal(compactionSummary('/reset', 'Reset complete.'), 'Conversation cleared.');
  assert.equal(compactionSummary('/compact', 'Earlier decisions and context.'), 'Earlier decisions and context.');
  assert.equal(compactionSummary('/compact', ''), 'Context compacted.');
});

test('resumePrompt restores prior roles only for a fresh conversation', () => {
  assert.equal(resumePrompt('next', []), 'next');
  assert.match(resumePrompt('next', [
    { role: 'user', text: 'before', timestamp: 1 },
    { role: 'assistant', text: 'answer', timestamp: 2 }
  ]), /user: before\nassistant: answer\n\nuser: next$/);
});

test('historyDelta catches up a backend without replaying turns it already knows', () => {
  const first = { role: 'user' as const, text: 'first', timestamp: 1 };
  const second = { role: 'assistant' as const, text: 'second', timestamp: 2 };
  assert.deepEqual(historyDelta([first], [first, second]), [second]);
  assert.equal(historyDelta([second], [first, second]), null);
});

test('ACP usage is attributed to the turn with a session-cost delta', () => {
  assert.deepEqual(toChatUsage(
    { inputTokens: 10, outputTokens: 5, totalTokens: 15 },
    null,
    { used: 12, size: 100, cost: { amount: 0.5, currency: 'USD' } },
    0.25,
    2500
  ), {
    input_tokens: 10,
    output_tokens: 5,
    total_tokens: 15,
    estimated_cost_usd: 0.25,
    duration_seconds: 2.5
  });
});

test('ACP cumulative counters are differenced without treating context occupancy as turn usage', () => {
  assert.deepEqual(toChatUsage(
    { inputTokens: 25, outputTokens: 12, totalTokens: 37 },
    { inputTokens: 10, outputTokens: 5, totalTokens: 15 },
    { used: 90, size: 100, cost: { amount: 0.75, currency: 'USD' } },
    0.5,
    1000
  ), {
    input_tokens: 15,
    output_tokens: 7,
    total_tokens: 22,
    estimated_cost_usd: 0.25,
    duration_seconds: 1
  });
  assert.equal(toChatUsage(undefined, null, { used: 90, size: 100 }, 0, 1000)?.total_tokens, null);
});

test('ACP model config drives the existing model picker', () => {
  assert.deepEqual(readModelConfig([{
    id: 'model',
    name: 'Model',
    category: 'model',
    type: 'select',
    currentValue: 'fast',
    options: [
      { value: 'fast', name: 'Fast' },
      { value: 'deep', name: 'Deep', description: 'More reasoning' }
    ]
  }]), {
    models: [
      { id: 'fast', name: 'Fast', description: undefined },
      { id: 'deep', name: 'Deep', description: 'More reasoning' }
    ],
    currentModelId: 'fast',
    configId: 'model'
  });
});

test('ACP thought-level config drives reasoning effort without inventing choices', () => {
  assert.deepEqual(readReasoningConfig([{
    id: 'reasoning_effort',
    name: 'Reasoning Effort',
    category: 'thought_level',
    type: 'select',
    currentValue: 'high',
    options: [
      { value: 'low', name: 'Low' },
      { value: 'high', name: 'High' },
      { value: 'ultra', name: 'Ultra', description: 'Backend-specific maximum' }
    ]
  }, {
    id: 'thinking',
    name: 'Non-standard thinking option',
    category: 'thinking',
    type: 'select',
    currentValue: 'made-up',
    options: [{ value: 'made-up', name: 'Made up' }]
  }]), {
    efforts: [
      { id: 'low', name: 'Low', description: undefined },
      { id: 'high', name: 'High', description: undefined },
      { id: 'ultra', name: 'Ultra', description: 'Backend-specific maximum' }
    ],
    currentEffortId: 'high',
    configId: 'reasoning_effort'
  });
});

test('toContextUsage passes through a valid usage_update payload (#865)', () => {
  assert.deepEqual(toContextUsage({ used: 12594, size: 131072 }), { used: 12594, size: 131072 });
});

test('toContextUsage hides absent, non-finite, or out-of-range usage_update data', () => {
  assert.equal(toContextUsage(null), null);
  assert.equal(toContextUsage({ used: NaN, size: 131072 }), null);
  assert.equal(toContextUsage({ used: 10, size: Infinity }), null);
  assert.equal(toContextUsage({ used: -1, size: 131072 }), null);
  assert.equal(toContextUsage({ used: 10, size: 0 }), null);
});

test('normalizeRemoteUrl collapses https/ssh/scp variants of the same remote', () => {
  assert.equal(normalizeRemoteUrl('https://github.com/Kh1ng/git-agent-harness.git'), 'github.com/kh1ng/git-agent-harness');
  assert.equal(normalizeRemoteUrl('git@github.com:Kh1ng/git-agent-harness.git'), 'github.com/kh1ng/git-agent-harness');
  assert.equal(normalizeRemoteUrl('ssh://git@gitlab.example.com/Khing/sportsball-bets.git'), 'gitlab.example.com/khing/sportsball-bets');
  assert.equal(normalizeRemoteUrl('https://token:x-oauth-basic@github.com/Kh1ng/repo.git'), 'github.com/kh1ng/repo');
});

test('normalizeRemoteUrl strips trailing slashes and is idempotent', () => {
  assert.equal(normalizeRemoteUrl('https://github.com/Kh1ng/repo/'), 'github.com/kh1ng/repo');
  const once = normalizeRemoteUrl('git@github.com:Kh1ng/repo.git');
  assert.equal(normalizeRemoteUrl(once), once);
});

test('model override persists per profile+backend and survives a fresh read', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-manager-chat-settings-'));
  const prevPath = process.env.GAH_MANAGER_CHAT_SETTINGS_PATH;
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(dir, 'settings.json');
  try {
    assert.equal(modelOverrideForProfile('gah', 'hermes'), undefined);
    setModelOverrideForProfile('gah', 'hermes', 'local-qwen');
    assert.equal(modelOverrideForProfile('gah', 'hermes'), 'local-qwen');
    // Different backend on the same profile is a distinct key.
    assert.equal(modelOverrideForProfile('gah', 'claude'), undefined);
    setModelOverrideForProfile('gah', 'hermes', 'deepseek/deepseek-v4-flash-0731');
    assert.equal(modelOverrideForProfile('gah', 'hermes'), 'deepseek/deepseek-v4-flash-0731');
  } finally {
    rmSync(dir, { recursive: true, force: true });
    if (prevPath === undefined) delete process.env.GAH_MANAGER_CHAT_SETTINGS_PATH;
    else process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = prevPath;
  }
});

test('reasoning effort override persists per profile+backend', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-manager-chat-reasoning-'));
  const prevPath = process.env.GAH_MANAGER_CHAT_SETTINGS_PATH;
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(dir, 'settings.json');
  try {
    assert.equal(reasoningEffortOverrideForProfile('gah', 'codex'), undefined);
    setReasoningEffortOverrideForProfile('gah', 'codex', 'xhigh');
    assert.equal(reasoningEffortOverrideForProfile('gah', 'codex'), 'xhigh');
    assert.equal(reasoningEffortOverrideForProfile('gah', 'opencode'), undefined);
    assert.equal(reasoningEffortOverrideForProfile('other', 'codex'), undefined);
  } finally {
    rmSync(dir, { recursive: true, force: true });
    if (prevPath === undefined) delete process.env.GAH_MANAGER_CHAT_SETTINGS_PATH;
    else process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = prevPath;
  }
});

test('isUsageLimitError classifies quota-limit messages but not auth/crash/network errors', () => {
  assert.equal(isUsageLimitError(new Error("You've hit your usage limit. Please wait or upgrade.")), true);
  assert.equal(isUsageLimitError(new Error('Rate limit exceeded, retry in 30s.')), true);
  assert.equal(isUsageLimitError(new Error('Quota exhausted: insufficient credits.')), true);
  assert.equal(isUsageLimitError(new Error('Token limit reached.')), false); // not a quota-limit trigger by itself
  assert.equal(isUsageLimitError(new Error('401 Unauthorized: invalid API key')), false);
  assert.equal(isUsageLimitError(new Error('backend crashed: segfault')), false);
  assert.equal(isUsageLimitError(new Error('network error: connection refused')), false);
});

test('handoffAttempt reruns a usage-limited turn on the next eligible backend, once', async () => {
  const calls: string[] = [];
  const result = await handoffAttempt({
    startBackend: 'hermes',
    fallbackBackends: ['codex', 'claude'],
    attempt: async (backendId) => {
      calls.push(backendId);
      if (backendId === 'hermes') throw new Error("You've hit your usage limit.");
      return { reply: `answered by ${backendId}`, model: null, usage: null };
    }
  });
  assert.deepEqual(calls, ['hermes', 'codex']);
  assert.equal(result.backend, 'codex');
  assert.equal(result.reply, 'answered by codex');
  assert.deepEqual(result.handoff, { from: 'hermes', to: 'codex', reason: "You've hit your usage limit." });
});

test('handoffAttempt skips a fallback that fails for a non-limit reason and tries the next', async () => {
  const result = await handoffAttempt({
    startBackend: 'hermes',
    fallbackBackends: ['codex', 'claude'],
    attempt: async (backendId) => {
      if (backendId === 'hermes') throw new Error('usage limit hit');
      if (backendId === 'codex') throw new Error('codex not installed');
      return { reply: 'claude answer', model: 'opus', usage: null };
    }
  });
  assert.equal(result.backend, 'claude');
  assert.deepEqual(result.handoff, { from: 'hermes', to: 'claude', reason: 'usage limit hit' });
});

test('handoffAttempt does not hand off on non-limit errors', async () => {
  const calls: string[] = [];
  await assert.rejects(
    handoffAttempt({
      startBackend: 'hermes',
      fallbackBackends: ['codex'],
      attempt: async (backendId) => {
        calls.push(backendId);
        throw new Error('401 Unauthorized: invalid API key');
      }
    }),
    /401 Unauthorized/
  );
  assert.deepEqual(calls, ['hermes'], 'no fallback was attempted');
});

test('handoffAttempt fails the turn when no fallback is configured', async () => {
  await assert.rejects(
    handoffAttempt({
      startBackend: 'hermes',
      fallbackBackends: [],
      attempt: async () => { throw new Error('usage limit hit'); }
    }),
    /usage limit hit/
  );
});

test('handoffAttempt allows at most one handoff: a second limit error fails the turn', async () => {
  await assert.rejects(
    handoffAttempt({
      startBackend: 'hermes',
      fallbackBackends: ['codex'],
      attempt: async (backendId) => {
        throw new Error(`${backendId} usage limit`);
      }
    }),
    /hermes usage limit/
  );
});
