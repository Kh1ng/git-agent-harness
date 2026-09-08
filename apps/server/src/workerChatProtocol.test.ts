import assert from 'node:assert/strict';
import test from 'node:test';
import { parseWorkerChatEvent, parseWorkerChatReply, readWorkerChatReply, type WorkerChatEvent } from './workerChatProtocol.js';

const usage = { input_tokens: 3, output_tokens: 2, total_tokens: 5, estimated_cost_usd: 0.01, duration_seconds: 0.5 };
const result = { type: 'result', result: { reply: 'Done', model: null, usage } };
const permission = { type: 'permission', id: 'permission-1', request: { title: 'Read file?', locations: ['/worker/file'], options: [{ optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' }] } };
const tool = { toolCallId: 'tool-1', name: null, title: 'Read file', kind: null, status: 'pending' as const, locations: ['/worker/file'], summary: null };

test('worker chat parser accepts all event variants and strips unrelated metadata', () => {
  const events: WorkerChatEvent[] = [
    { type: 'chunk', text: 'Hello' }, { type: 'toolResult', name: 'read', text: 'contents' },
    { type: 'toolCall', tool }, { type: 'permission', id: permission.id, request: permission.request },
    { type: 'result', result: result.result }, { type: 'error', error: 'Stopped' }
  ];
  for (const event of events) assert.deepEqual(parseWorkerChatEvent({ ...event, unexpected_token: 'secret' }), event);
  assert.deepEqual(parseWorkerChatEvent({ ...result, result: { ...result.result, usage: null } }), { ...result, result: { ...result.result, usage: null } });
  const emptyUsage = Object.fromEntries(Object.keys(usage).map((key) => [key, null]));
  assert.deepEqual(parseWorkerChatEvent({ ...result, result: { ...result.result, model: 'worker-model', usage: { ...emptyUsage, secret: 'omitted' } } }),
    { ...result, result: { ...result.result, model: 'worker-model', usage: emptyUsage } });
});

test('usage fields must be present, nullable, finite and nonnegative; token counts must be integral', () => {
  for (const key of Object.keys(usage)) {
    for (const value of [undefined, -1, NaN, Infinity, '3', false]) {
      assert.throws(() => parseWorkerChatEvent({ ...result, result: { ...result.result, usage: { ...usage, [key]: value } } }), /invalid chat event/, key);
    }
  }
  for (const key of ['input_tokens', 'output_tokens', 'total_tokens']) {
    assert.throws(() => parseWorkerChatEvent({ ...result, result: { ...result.result, usage: { ...usage, [key]: 1.5 } } }), /invalid chat event/, key);
  }
  for (const value of [undefined, 1, {}]) assert.throws(() => parseWorkerChatEvent({ ...result, result: { ...result.result, model: value } }), /invalid chat event/);
  assert.throws(() => parseWorkerChatEvent({ type: 'result', result: { reply: 'Done', model: null } }), /invalid chat event/);
});

test('permission choices and tool activity must have the complete consumed shape', () => {
  for (const options of [[], [null], ['allow'], [{ optionId: 'allow' }], [{ ...permission.request.options[0], name: null }], [permission.request.options[0], permission.request.options[0]]]) {
    assert.throws(() => parseWorkerChatEvent({ ...permission, request: { ...permission.request, options } }), /invalid chat event/);
  }
  for (const patch of [{ status: 'unknown' }, { locations: [null] }, { title: null }, { kind: 3 }, { name: undefined }, { summary: undefined }, { toolCallId: '' }]) {
    assert.throws(() => parseWorkerChatEvent({ type: 'toolCall', tool: { ...tool, ...patch } }), /invalid chat event/);
  }
  for (const value of [null, [], {}, { type: 'unknown' }, { type: 'chunk', text: 1 }, { type: 'error' }, { type: 'toolResult', name: 'read' }]) {
    assert.throws(() => parseWorkerChatEvent(value), /invalid chat event/);
  }
});

const session = { id: 'session-1', profile: 'worker-project', branch: 'gah-chat-session-1', worktreePath: '/worker/checkout',
  backend: 'codex', model: null, reasoningEffort: null, title: null, createdAt: 100, lastActiveAt: 100,
  archivedAt: null, outcome: 'live', settledAt: null, settledReason: null };
const create = { action: 'create', profile: session.profile, backend: session.backend, sessionId: session.id };
const models = { models: [{ id: 'model-1', name: 'Model', description: 'Available model' }], currentModelId: 'model-1',
  reasoningEfforts: [{ id: 'high', name: 'High' }], currentReasoningEffortId: null, contextUsage: { size: 100, used: 10 } };

test('worker session replies preserve expected identity and omit worker-supplied central metadata', () => {
  const injected = { ...session, nodeId: 'central', workspaceNodes: ['central'], remoteWorkspace: false,
    workspaces: { central: { branch: 'main', worktreePath: '/central/checkout' } }, prNumber: 99, secret: 'omit' };
  assert.deepEqual(parseWorkerChatReply(injected, create), session);
  assert.deepEqual(parseWorkerChatReply({ session: injected, extra: 'omit' }, { ...create, action: 'prepare' }), { session });
  const archived = { ...session, archivedAt: 101, outcome: 'archived' };
  assert.deepEqual(parseWorkerChatReply(archived, { ...create, action: 'archive' }), archived);
  for (const patch of [{ id: 'different' }, { id: '../escape' }, { profile: 'other-project' }, { backend: 'other-backend' },
    { worktreePath: {} }, { model: undefined }, { reasoningEffort: [] }, { title: 1 }, { branch: '' },
    { createdAt: -1 }, { lastActiveAt: Infinity }, { archivedAt: 101 }, { settledAt: 101 }, { settledReason: 'merged' },
    { outcome: ['live'] }]) {
    assert.throws(() => parseWorkerChatReply({ ...session, ...patch }, create), /invalid chat response/);
  }
  assert.throws(() => parseWorkerChatReply({ session: { ...session, id: 'other' } }, { ...create, action: 'prepare' }), /invalid chat response/);
  assert.throws(() => parseWorkerChatReply({ session: null }, { ...create, action: 'prepare' }), /invalid chat response/);
  assert.throws(() => parseWorkerChatReply(session, { ...create, action: 'archive' }), /invalid chat response/);
  assert.throws(() => parseWorkerChatReply({ ...archived, outcome: ['archived'] }, { ...create, action: 'archive' }), /invalid chat response/);
});

test('worker model and command responses validate consumed fields and strip unknown values', () => {
  assert.deepEqual(parseWorkerChatReply({ ...models, models: [{ ...models.models[0], secret: 'omit' }],
    reasoningEfforts: [{ ...models.reasoningEfforts[0], secret: 'omit' }], contextUsage: { ...models.contextUsage, secret: 'omit' } }, { action: 'models' }), models);
  const commands = [{ name: 'help', description: 'Help', argsHint: 'topic' }];
  assert.deepEqual(parseWorkerChatReply([{ ...commands[0], secret: 'omit' }], { action: 'commands' }), commands);
  assert.deepEqual(parseWorkerChatReply({ success: true, secret: 'omit' }, { action: 'cancel' }), { success: true });
  for (const patch of [{ models: [null] }, { models: [{ id: '', name: 'bad' }] }, { models: [models.models[0], models.models[0]] },
    { models: [{ ...models.models[0], description: null }] }, { reasoningEfforts: [{ id: 'high' }] },
    { currentModelId: {} }, { currentReasoningEffortId: undefined }, { contextUsage: undefined },
    { contextUsage: { size: 100, used: -1 } }, { contextUsage: { size: Infinity, used: 10 } }]) {
    assert.throws(() => parseWorkerChatReply({ ...models, ...patch }, { action: 'models' }), /invalid chat response/);
  }
  for (const value of [null, {}, [null], [{ name: 'help' }], [{ name: 'help', description: 'Help', argsHint: null }]]) {
    assert.throws(() => parseWorkerChatReply(value, { action: 'commands' }), /invalid chat response/);
  }
  assert.throws(() => parseWorkerChatReply({ success: false }, { action: 'cancel' }), /invalid chat response/);
  assert.throws(() => parseWorkerChatReply({ success: true }, { action: 'unknown' }), /invalid chat response/);
});

test('worker JSON reads enforce a byte limit and cancel oversized streams without trusting Content-Length', async () => {
  assert.deepEqual(await readWorkerChatReply(Response.json(session), create), session);
  await assert.rejects(readWorkerChatReply(new Response('not JSON'), create), /invalid chat response/);
  await assert.rejects(readWorkerChatReply(new Response('{', { headers: { 'Content-Type': 'application/json' } }), create), /invalid chat response/);
  let cancelled = false;
  let reads = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) { reads++; controller.enqueue(new Uint8Array(1_000_001)); },
    cancel() { cancelled = true; }
  });
  await assert.rejects(readWorkerChatReply(new Response(body, { headers: { 'Content-Type': 'application/json', 'Content-Length': '1' } }), create), /exceeds the supported size/);
  assert.equal(cancelled, true);
  assert.ok(reads <= 3, 'does not buffer the entire response');
});
