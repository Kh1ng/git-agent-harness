import { taskReasoningEfforts } from './agentModelOptions.js';

const levels = (ids: string[]) => ids.map((id) => ({ id, name: id }));

test('drops advertised levels the native CLI flag cannot carry', () => {
  expect(taskReasoningEfforts('codex', levels(['default', 'minimal', 'low', 'high', 'ultra', 'turbo'])).map((effort) => effort.id))
    .toEqual(['default', 'low', 'high', 'ultra']);
  expect(taskReasoningEfforts('claude', levels(['low', 'max', 'ultra'])).map((effort) => effort.id)).toEqual(['low', 'max']);
});

test('falls back to the Claude CLI levels when the catalog advertises none', () => {
  expect(taskReasoningEfforts('claude').map((effort) => effort.id)).toEqual(['low', 'medium', 'high', 'xhigh', 'max']);
  expect(taskReasoningEfforts('codex')).toEqual([]);
});
