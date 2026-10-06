import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { agentModel, agentSessionId, agentTool, claudeDescriber, classifyAgents, describeClaudeSession, deviceAgentsSnapshot } from './deviceAgents.js';

test('agentTool recognises agent CLIs run directly or through a launcher', () => {
  assert.equal(agentTool(['claude']), 'claude');
  assert.equal(agentTool(['/home/me/.local/bin/codex', 'exec', 'fix the bug']), 'codex');
  assert.equal(agentTool(['node', '/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js']), null);
  assert.equal(agentTool(['node', '/usr/local/bin/claude', '--resume']), 'claude');
  assert.equal(agentTool(['python3', '/opt/tools/aider.py']), 'aider');
  // A prompt that mentions an agent is not that agent.
  assert.equal(agentTool(['bash', '-c', 'claude --help']), null);
  assert.equal(agentTool(['vim', 'codex.md']), null);
  assert.equal(agentTool([]), null);
});

test('agentModel reads only the model flag', () => {
  assert.equal(agentModel(['claude', '--model', 'claude-opus-5-5[1m]', '--verbose']), 'claude-opus-5-5[1m]');
  assert.equal(agentModel(['codex', 'exec', '-m', 'gpt-6-sol', 'a prompt']), 'gpt-6-sol');
  assert.equal(agentModel(['vibe', '--model=glm-5-3']), 'glm-5-3');
  assert.equal(agentModel(['codex', 'exec', 'please use --model wisely']), null);
  assert.equal(agentModel(['claude', '--model', 'not a model; rm -rf']), null);
});

test('classifyAgents splits factory worktrees from everything else and folds helper processes', () => {
  const result = classifyAgents([
    { pid: 10, ppid: 1, argv: ['claude'], cwd: '/home/me/Projects/site', startedAt: '2026-10-04T10:00:00.000Z' },
    { pid: 11, ppid: 10, argv: ['node', '/usr/bin/claude', '--mcp'], cwd: '/home/me/Projects/site', startedAt: '2026-10-04T10:00:05.000Z' },
    // A second conversation in the same directory is its own row.
    { pid: 12, ppid: 1, argv: ['claude'], cwd: '/home/me/Projects/site', startedAt: '2026-10-04T10:30:00.000Z' },
    { pid: 20, ppid: 5, argv: ['codex', 'exec', '--model', 'gpt-6-sol', 'secret prompt'], cwd: '/home/me/worktrees/gah-123', startedAt: '2026-10-04T11:00:00.000Z' },
    { pid: 21, ppid: 1, argv: ['codex'], cwd: '/home/me/worktrees-other', startedAt: '2026-10-04T12:00:00.000Z' },
    { pid: 30, ppid: null, argv: ['claude'], cwd: null, startedAt: null },
    { pid: 40, ppid: 1, argv: ['bash'], cwd: '/home/me', startedAt: '2026-10-04T09:00:00.000Z' },
    { pid: 99, ppid: 1, argv: ['claude'], cwd: '/srv', startedAt: '2026-10-04T13:00:00.000Z' }
  ], ['/home/me/worktrees', '', '/'], 99);
  assert.deepEqual(result.agents, [
    { pid: 21, tool: 'codex', cwd: '/home/me/worktrees-other', started_at: '2026-10-04T12:00:00.000Z', model: null },
    { pid: 12, tool: 'claude', cwd: '/home/me/Projects/site', started_at: '2026-10-04T10:30:00.000Z', model: null },
    { pid: 10, tool: 'claude', cwd: '/home/me/Projects/site', started_at: '2026-10-04T10:00:00.000Z', model: null },
    { pid: 30, tool: 'claude', cwd: null, started_at: null, model: null }
  ]);
  assert.deepEqual(result.factory_agents, [
    { pid: 20, tool: 'codex', cwd: '/home/me/worktrees/gah-123', started_at: '2026-10-04T11:00:00.000Z', model: 'gpt-6-sol' }
  ]);
  // The command line never leaves the scanner.
  assert.ok(!JSON.stringify(result).includes('secret prompt'));
});

test('deviceAgentsSnapshot reads this host without throwing', () => {
  const snapshot = deviceAgentsSnapshot([]);
  assert.equal(snapshot.supported, process.platform === 'linux');
  assert.ok(Array.isArray(snapshot.agents) && Array.isArray(snapshot.factory_agents));
  const allowed = new Set(['cwd', 'last_activity_at', 'model', 'pid', 'started_at', 'title', 'tool']);
  for (const agent of [...snapshot.agents, ...snapshot.factory_agents]) assert.ok(Object.keys(agent).every((key) => allowed.has(key)));
});

test('a Claude session is described by its title record and last write, never its prompts', () => {
  const id = '5080330c-60fd-40ec-a3e7-847c522c3af5';
  assert.equal(agentSessionId(['claude', '--verbose', `--resume=${id}`]), id);
  assert.equal(agentSessionId(['claude', '--session-id', id]), id);
  assert.equal(agentSessionId(['claude', '--resume', '../../etc/passwd']), null);
  assert.equal(agentSessionId(['claude']), null);

  const home = mkdtempSync(join(tmpdir(), 'gah-device-agents-'));
  mkdirSync(join(home, '.claude', 'projects', '-home-me-site'), { recursive: true });
  writeFileSync(join(home, '.claude', 'projects', '-home-me-site', `${id}.jsonl`), [
    JSON.stringify({ type: 'user', message: { content: 'my secret prompt' } }),
    JSON.stringify({ type: 'ai-title', aiTitle: 'First title', sessionId: id }),
    JSON.stringify({ type: 'assistant', message: { content: 'mentions "ai-title" in passing' } }),
    JSON.stringify({ type: 'ai-title', aiTitle: 'Fix the checkout flow', sessionId: id })
  ].join('\n'));
  const described = describeClaudeSession(id, home);
  assert.equal(described?.title, 'Fix the checkout flow');
  assert.ok(Date.now() - Date.parse(described!.last_activity_at!) < 60_000);
  assert.equal(describeClaudeSession('00000000-0000-4000-8000-000000000000', home), null);

  const { agents, factory_agents } = classifyAgents([
    { pid: 1, ppid: 0, argv: ['claude', `--resume=${id}`], cwd: '/home/me/site', startedAt: null },
    { pid: 2, ppid: 0, argv: ['claude', `--resume=${id}`], cwd: '/w/job', startedAt: null }
  ], ['/w'], 99, claudeDescriber(home));
  assert.equal(agents[0].title, 'Fix the checkout flow');
  assert.equal(factory_agents[0].title, undefined);
  assert.ok(!JSON.stringify(agents).includes('secret prompt'));
});

test('a session started without an id is found by its start time; default resolves to the real model', () => {
  const home = mkdtempSync(join(tmpdir(), 'gah-device-agents-'));
  const project = join(home, '.claude', 'projects', '-home-me-my-site');
  mkdirSync(project, { recursive: true });
  const mine = '11111111-1111-4111-8111-111111111111';
  const other = '22222222-2222-4222-8222-222222222222';
  writeFileSync(join(project, `${mine}.jsonl`), [
    JSON.stringify({ type: 'queue-operation', timestamp: '2026-10-04T22:34:49.354Z', sessionId: mine }),
    JSON.stringify({ type: 'assistant', message: { model: 'claude-opus-5-5', content: [] } }),
    JSON.stringify({ type: 'assistant', message: { model: 'claude-fable-5-1', content: [] } })
  ].join('\n'));
  writeFileSync(join(project, `${other}.jsonl`), JSON.stringify({ type: 'queue-operation', timestamp: '2026-10-04T18:00:00.000Z', sessionId: other }));
  // The desktop app's record supplies the thread name people see there.
  const desktop = join(home, '.config', 'Claude', 'claude-code-sessions', 'account', 'workspace');
  mkdirSync(desktop, { recursive: true });
  writeFileSync(join(desktop, 'local_a.json'), JSON.stringify({ cliSessionId: mine, title: 'Fix GAH dashboard glitches', model: 'claude-fable-5-1' }));

  const { agents } = classifyAgents([
    { pid: 1, ppid: 0, argv: ['claude', '--model', 'default'], cwd: '/home/me/my.site', startedAt: '2026-10-04T22:34:47.560Z' },
    { pid: 2, ppid: 0, argv: ['claude', '--model', 'default'], cwd: '/home/me/elsewhere', startedAt: '2026-10-04T22:34:47.560Z' }
  ], [], 99, claudeDescriber(home));
  assert.equal(agents.find((agent) => agent.pid === 1)?.title, 'Fix GAH dashboard glitches');
  assert.equal(agents.find((agent) => agent.pid === 1)?.model, 'claude-fable-5-1');
  // Nothing known: no invented model, and never the word "default".
  assert.equal(agents.find((agent) => agent.pid === 2)?.model, null);
});
