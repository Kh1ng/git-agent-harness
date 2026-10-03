import assert from 'node:assert/strict';
import { test } from 'node:test';
import { instanceAcpSpawn, instanceHeadlessSpec, bindOpenCodeModels } from './instanceLaunch.js';
import type { BackendInstanceRuntime } from '../gahCli.js';
import type { ManagerAdapter } from './registry.js';

const runtime = (runner: string): BackendInstanceRuntime => ({ backend_instance: `${runner}-work`, runner_kind: runner,
  logical_backend: runner, executable: `/tools/${runner}`, state_root: '/isolated/work', account_label: 'Work',
  credential_id: 'work', credential_provider: 'nous', credential_revision: 'generation-2' });

test('named ACP launches retain owner HOME for secret lookup and use local instance references', () => {
  for (const runner of ['codex', 'claude', 'opencode', 'hermes']) {
    const bridged = runner === 'codex' || runner === 'claude';
    const result = instanceAcpSpawn('profile', runtime(runner), {
      command: bridged ? 'node' : `/tools/${runner}`,
      args: bridged ? ['/bridges/acp.js'] : ['acp'],
      env: { HOME: '/isolated/work', CODEX_HOME: '/isolated/work/.codex',
        CLAUDE_CONFIG_DIR: '/isolated/work/.claude', XDG_DATA_HOME: '/isolated/work/data',
        OPENCODE_CONFIG_CONTENT: '{"default_agent":"gah-implementer"}' }
    });
    assert.deepEqual(result.args, ['config', 'exec-backend-instance', '--profile', 'profile', '--instance', `${runner}-work`,
      ...(bridged ? ['--acp-bridge', '/bridges/acp.js', '--'] : ['--', 'acp'])]);
    assert.equal(result.env?.HOME, undefined);
    assert.equal(result.env?.CODEX_HOME, undefined);
    assert.equal(result.env?.CLAUDE_CONFIG_DIR, undefined);
    assert.equal(result.env?.XDG_DATA_HOME, undefined);
    assert.equal(result.env?.OPENCODE_CONFIG_CONTENT, '{"default_agent":"gah-implementer"}');
    assert.ok(!JSON.stringify(result).includes('generation-2'));
  }
});

test('named headless launches preserve stdin bridge and model metadata without copying keys', async () => {
  for (const runner of ['vibe', 'agy', 'openhands']) {
    const spec = instanceHeadlessSpec('profile', runtime(runner), {
      id: runner, displayName: runner,
      turnArgs: () => runner === 'vibe' ? ['/tools/python', '-c', 'fixed_stdin_bridge'] : [`/tools/${runner}`, '--print'],
      encodeStdin: prompt => prompt,
      spawnEnv: async () => ({ HOME: '/isolated/work', LLM_BASE_URL: 'https://inference.example/v1' })
    });
    const args = spec.turnArgs({ model: 'openai/gpt' });
    assert.deepEqual(args.slice(1), ['config', 'exec-backend-instance', '--profile', 'profile', '--instance', `${runner}-work`,
      '--model', 'openai/gpt', ...(runner === 'vibe' ? ['--adapter-program', '/tools/python', '--', '-c', 'fixed_stdin_bridge'] : ['--', '--print'])]);
    assert.equal(spec.encodeStdin('private transcript'), 'private transcript');
    assert.ok(!args.includes('private transcript'));
    assert.deepEqual(await spec.spawnEnv?.('profile'), { LLM_BASE_URL: 'https://inference.example/v1' });
  }
});

test('bound OpenCode filters billing providers and selects its account before an unpinned turn', async () => {
  const selected: string[] = [];
  const turns: string[] = [];
  const source = {
    id: 'opencode', displayName: 'OpenCode', implemented: true,
    listModels: async () => ({ models: [{ id: 'openai/gpt', name: 'Direct OpenAI' }, { id: 'nous-portal/openai/gpt', name: 'Nous GPT' }],
      currentModelId: 'openai/gpt', reasoningEfforts: [], currentReasoningEffortId: null, contextUsage: null }),
    setModel: async (_profile: string, model: string) => { selected.push(model); },
    listCommands: async () => [], setReasoningEffort: async () => {},
    steerTurn: async () => ({ outcome: 'injected' }), cancelTurn: async () => {},
    runTurn: async (_profile: string, input: { model?: string | null }) => {
      turns.push(input.model!); return { reply: 'ok', model: input.model!, usage: null };
    }
  } as ManagerAdapter;
  const adapter = bindOpenCodeModels(source, runtime('opencode'));
  assert.deepEqual((await adapter.listModels('p')).models.map(model => model.id), ['nous-portal/openai/gpt']);
  assert.equal((await adapter.listModels('p')).currentModelId, null);
  await assert.rejects(adapter.setModel('p', 'openai/gpt'), /different provider/);
  const input = { prompt: 'hello', history: [], onChunk: () => {}, onToolResult: () => {} };
  await assert.rejects(adapter.runTurn('p', { ...input, model: 'openai/gpt' }), /different provider/);
  await assert.rejects(adapter.runTurn('p', { ...input, model: 'nous-portal/missing' }), /no longer available/);
  await assert.rejects(adapter.setModel('p', 'nous-portal/missing'), /no longer available/);
  await adapter.runTurn('p', input);
  assert.deepEqual(selected, ['nous-portal/openai/gpt']);
  assert.deepEqual(turns, ['nous-portal/openai/gpt']);
  assert.equal(bindOpenCodeModels(source, runtime('hermes')), source, 'Hermes model vendor does not identify its billing provider');
});

test('bound OpenCode rejects unpinned turns with no matching provider model', async () => {
  const source = { listModels: async () => ({ models: [], currentModelId: null }) } as unknown as ManagerAdapter;
  const adapter = bindOpenCodeModels(source, runtime('opencode'));
  await assert.rejects(adapter.runTurn('p', { prompt: 'hello', history: [], onChunk: () => {}, onToolResult: () => {} }), /no model/);
});
