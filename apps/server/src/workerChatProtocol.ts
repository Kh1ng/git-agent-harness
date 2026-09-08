import type { ManagerAdapter } from './managerChat/registry.js';

type TurnInput = Parameters<ManagerAdapter['runTurn']>[1];
export type WorkerChatEvent =
  | { type: 'chunk'; text: string }
  | { type: 'toolResult'; name: string; text: string }
  | { type: 'toolCall'; tool: Parameters<NonNullable<TurnInput['onToolCall']>>[0] }
  | { type: 'permission'; id: string; request: Parameters<NonNullable<TurnInput['requestPermission']>>[0] }
  | { type: 'result'; result: Awaited<ReturnType<ManagerAdapter['runTurn']>> }
  | { type: 'error'; error: string };

const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every((entry) => typeof entry === 'string');
const nullableString = (value: unknown): value is string | null => value === null || typeof value === 'string';
const measurement = (value: unknown, integral = false): value is number | null => value === null
  || (typeof value === 'number' && Number.isFinite(value) && value >= 0 && (!integral || Number.isSafeInteger(value)));

/** Reject malformed worker output before it reaches permissions, UI state, or usage accounting.
 * Return only protocol fields so unrelated worker metadata is never persisted by central. */
export function parseWorkerChatEvent(value: unknown): WorkerChatEvent {
  if (object(value)) {
    if (value.type === 'chunk' && typeof value.text === 'string') return { type: 'chunk', text: value.text };
    if (value.type === 'error' && typeof value.error === 'string') return { type: 'error', error: value.error };
    if (value.type === 'toolResult' && typeof value.name === 'string' && typeof value.text === 'string') return { type: 'toolResult', name: value.name, text: value.text };
    if (value.type === 'toolCall' && object(value.tool)) {
      const tool = value.tool;
      if (typeof tool.toolCallId === 'string' && tool.toolCallId && typeof tool.title === 'string'
        && nullableString(tool.name) && nullableString(tool.kind) && nullableString(tool.summary) && strings(tool.locations)
        && (tool.status === 'pending' || tool.status === 'completed' || tool.status === 'failed')) {
        return { type: 'toolCall', tool: { toolCallId: tool.toolCallId, name: tool.name, title: tool.title,
          kind: tool.kind, status: tool.status, locations: tool.locations, summary: tool.summary } };
      }
    }
    if (value.type === 'permission' && typeof value.id === 'string' && value.id && object(value.request)) {
      const request = value.request;
      if (typeof request.title === 'string' && strings(request.locations) && Array.isArray(request.options)
        && request.options.length > 0 && request.options.every((option) => object(option)
          && typeof option.optionId === 'string' && option.optionId && typeof option.name === 'string' && typeof option.kind === 'string')
        && new Set(request.options.map((option) => option.optionId)).size === request.options.length) {
        return { type: 'permission', id: value.id, request: { title: request.title, locations: request.locations,
          options: request.options.map((option) => ({ optionId: option.optionId, name: option.name, kind: option.kind })) } };
      }
    }
    if (value.type === 'result' && object(value.result)) {
      const result = value.result;
      if (typeof result.reply === 'string' && nullableString(result.model)) {
        if (result.usage === null) return { type: 'result', result: { reply: result.reply, model: result.model, usage: null } };
        const usage = result.usage;
        if (object(usage) && measurement(usage.input_tokens, true) && measurement(usage.output_tokens, true)
          && measurement(usage.total_tokens, true) && measurement(usage.estimated_cost_usd) && measurement(usage.duration_seconds)) {
          return { type: 'result', result: { reply: result.reply, model: result.model, usage: {
            input_tokens: usage.input_tokens, output_tokens: usage.output_tokens, total_tokens: usage.total_tokens,
            estimated_cost_usd: usage.estimated_cost_usd, duration_seconds: usage.duration_seconds
          } } };
        }
      }
    }
  }
  throw new Error('Worker returned an invalid chat event.');
}
