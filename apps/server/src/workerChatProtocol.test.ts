import assert from 'node:assert/strict';
import test from 'node:test';
import { parseWorkerChatEvent, type WorkerChatEvent } from './workerChatProtocol.js';

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
