import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { compactionSummary, isCompactionCommand } from './managerChat/ManagerChatManager.js';
import { normalizeRemoteUrl } from './managerChat/memoryGatewayClient.js';
import { modelOverrideForProfile, setModelOverrideForProfile } from './managerChat/settingsStore.js';
import { historyDelta, readModelConfig, resumePrompt, toChatUsage } from './managerChat/acpAdapter.js';

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
