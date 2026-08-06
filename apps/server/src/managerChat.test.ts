import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { isCompactionCommand } from './managerChat/ManagerChatManager.js';
import { normalizeRemoteUrl } from './managerChat/memoryGatewayClient.js';
import { modelOverrideForProfile, setModelOverrideForProfile } from './managerChat/settingsStore.js';

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
