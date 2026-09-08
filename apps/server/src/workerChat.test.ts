import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import type { ManagerAdapter } from './managerChat/registry.js';
import { createWorkerChatRouter } from './workerChat.js';
import { workerChatConnection } from './remoteChat.js';
import { RegistryService } from './registryService.js';
import { getSession } from './managerChat/chatSessions.js';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

test('remote agent runs only in the worker profile and streams permission, steering, cancellation and replies', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'gah-worker-chat-'));
  const cwd = join(root, 'worker-checkout');
  mkdirSync(cwd);
  const profile: ProfileSummary = { ...JSON.parse(readFileSync(new URL('../tests/fixtures/gah/responses/profile-list.json', import.meta.url), 'utf8'))[0], name: 'demo', local_path: cwd, worktree_base: '' };
  let calls = 0;
  let cancelled = 0;
  let steered = '';
  let entered!: () => void;
  const running = new Promise<void>(resolve => { entered = resolve; });
  const adapter: ManagerAdapter = {
    id: 'claude', displayName: 'Claude', implemented: true,
    async runTurn(key, input) {
      calls++;
      assert.equal(key, 'demo#test-session');
      assert.equal(input.cwd, cwd);
      if (input.prompt === 'wait') { entered(); return new Promise(() => {}); }
      input.onChunk('streamed ');
      input.onToolResult('read', 'worker file');
      const choice = await input.requestPermission!({ title: 'Read worker file?', locations: [], options: [{ optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' }] });
      assert.equal(choice, 'allow-once');
      return { reply: 'worker reply', model: null, usage: null };
    },
    listCommands: async () => [],
    listModels: async () => ({ models: [], currentModelId: null, reasoningEfforts: [], currentReasoningEffortId: null, contextUsage: null }),
    setModel: async () => {}, setReasoningEffort: async () => {},
    steerTurn: async (_key, message) => { steered = message; return { outcome: 'injected' }; },
    cancelTurn: async () => { cancelled++; }
  };
  const app = express();
  app.use(express.json());
  app.use('/api/worker-chat', createWorkerChatRouter({ node: { role: 'worker', central_url: 'https://central.test' }, profiles: async () => [profile], adapter: () => adapter, sessions: { stateDir: join(root, 'worker-state') } }));
  const server = createServer(app);
  t.after(async () => {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(root, { recursive: true, force: true });
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const url = `http://127.0.0.1:${address.port}`;
  const registry = new RegistryService(join(root, 'central-registry.json'));
  registry.registerNode({ node_id: 'worker', display_name: 'Worker', advertised_url: url, version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST, transport_mode: 'loopback', secret_ref: 'env:WORKER_CHAT_TEST_TOKEN', profiles: ['demo'] });
  const connection = workerChatConnection(registry, 'worker', 'demo');
  const remote = connection.adapter('claude', 'test-session');
    const bad = await fetch(`${url}/api/worker-chat`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'run', profile: 'demo', sessionId: 'bad-turn', backend: 'claude', prompt: 'bad' }) });
    assert.equal(bad.status, 400);
    assert.equal(getSession('demo', 'bad-turn', { stateDir: join(root, 'worker-state') }), null, 'invalid input must not create a workspace');
    const chunks: string[] = [];
    const result = await remote.runTurn('central-conversation-key', {
      prompt: 'read', history: [], cwd: '/forbidden-central-path',
      onChunk: chunk => chunks.push(chunk), onToolResult: (name, text) => chunks.push(`${name}:${text}`),
      requestPermission: async request => { assert.equal(request.title, 'Read worker file?'); return 'allow-once'; }
    });
    assert.deepEqual(result, { reply: 'worker reply', model: null, usage: null });
    assert.deepEqual(chunks, ['streamed ', 'read:worker file']);
    assert.equal(calls, 1);
    const pending = remote.runTurn('central-conversation-key', { prompt: 'wait', history: [], onChunk() {}, onToolResult() {} });
    const stopped = assert.rejects(pending);
    await running;
    await remote.steerTurn('central-conversation-key', 'use the other file');
    assert.equal(steered, 'use the other file');
    await remote.cancelTurn('central-conversation-key');
    await stopped;
    assert.equal(cancelled, 1);
    assert.throws(() => workerChatConnection(registry, 'worker', 'not-declared'), /not registered/);
});
