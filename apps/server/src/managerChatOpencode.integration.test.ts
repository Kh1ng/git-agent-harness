import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { resolveAdapter } from './managerChat/registry.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

test('OpenCode Manager Chat overrides a review-only default for its isolated ACP child', { timeout: 10_000 }, async () => {
  const worktree = mkdtempSync(join(tmpdir(), 'gah-manager-chat-opencode-'));
  const previousPath = process.env.PATH;
  const previousConfig = process.env.OPENCODE_CONFIG_CONTENT;
  const previousDefaultAgent = process.env.GAH_TEST_OPENCODE_DEFAULT_AGENT;
  const inheritedConfig = JSON.stringify({
    model: 'provider/preserved-model'
  });
  process.env.PATH = `${join(fixtures, 'opencode')}:${previousPath ?? ''}`;
  process.env.OPENCODE_CONFIG_CONTENT = inheritedConfig;
  process.env.GAH_TEST_OPENCODE_DEFAULT_AGENT = 'gah-reviewer';
  writeFileSync(join(worktree, 'README.md'), '# isolated worktree\n');

  const toolCalls: string[] = [];
  try {
    const result = await resolveAdapter('opencode').runTurn(`opencode-${Date.now()}`, {
      prompt: 'Read README.md and create manager-chat-edit.txt.',
      history: [],
      cwd: worktree,
      onChunk: () => {},
      onToolResult: () => {},
      onToolCall: (tool) => toolCalls.push(`${tool.name}:${tool.status}`)
    });

    assert.equal(result.reply, 'Read and edited the bound worktree.');
    assert.equal(readFileSync(join(worktree, 'manager-chat-edit.txt'), 'utf8'), 'edited by gah-implementer\n');
    assert.deepEqual(toolCalls, [
      'read:pending',
      'read:completed',
      'edit:pending',
      'edit:completed'
    ]);
    assert.equal(
      process.env.OPENCODE_CONFIG_CONTENT,
      inheritedConfig,
      'Manager Chat must not mutate the parent/global OpenCode configuration'
    );
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousConfig === undefined) delete process.env.OPENCODE_CONFIG_CONTENT;
    else process.env.OPENCODE_CONFIG_CONTENT = previousConfig;
    if (previousDefaultAgent === undefined) delete process.env.GAH_TEST_OPENCODE_DEFAULT_AGENT;
    else process.env.GAH_TEST_OPENCODE_DEFAULT_AGENT = previousDefaultAgent;
    if (existsSync(worktree)) rmSync(worktree, { recursive: true, force: true });
  }
});
