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

type WorkerChatReply = import('@git-agent-harness/contracts').ChatSessionSummary
  | { session: import('@git-agent-harness/contracts').ChatSessionSummary }
  | Awaited<ReturnType<ManagerAdapter['listModels']>>
  | Awaited<ReturnType<ManagerAdapter['listCommands']>>
  | { success: true };
const timestamp = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
const optionalString = (value: unknown): value is string | undefined => value === undefined || typeof value === 'string';

/** A worker owns its checkout, never central's session identity or node metadata.
 * Project only the action's response fields before central stores or renders them. */
export function parseWorkerChatReply(value: unknown, request: Record<string, unknown>): WorkerChatReply {
  const invalid = () => new Error('Worker returned an invalid chat response.');
  if (request.action === 'create' || request.action === 'prepare' || request.action === 'archive') {
    const session = request.action === 'prepare' && object(value) ? value.session : value;
    if (!object(session) || typeof session.id !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(session.id)
      || typeof request.profile !== 'string' || session.profile !== request.profile
      || (request.sessionId !== undefined && session.id !== request.sessionId)
      || (request.action !== 'create' && typeof request.sessionId !== 'string')
      || typeof session.backend !== 'string' || !session.backend
      || (request.action !== 'archive' && session.backend !== request.backend)
      || typeof session.branch !== 'string' || !session.branch || !nullableString(session.worktreePath)
      || !nullableString(session.model) || !nullableString(session.reasoningEffort) || !nullableString(session.title)
      || !timestamp(session.createdAt) || !timestamp(session.lastActiveAt)
      || !(session.archivedAt === null || timestamp(session.archivedAt))
      || !(session.settledAt === null || timestamp(session.settledAt))
      || !(session.outcome === 'live' || session.outcome === 'archived' || session.outcome === 'settled')
      || !(session.settledReason === null || session.settledReason === 'merged' || session.settledReason === 'closed' || session.settledReason === 'delivered')
      || (request.action !== 'archive' && (session.outcome !== 'live' || session.archivedAt !== null || session.settledAt !== null || session.settledReason !== null))
      || (request.action === 'archive' && (session.outcome === 'live' || session.archivedAt === null))) throw invalid();
    const result: import('@git-agent-harness/contracts').ChatSessionSummary = {
      id: session.id, profile: request.profile, branch: session.branch, worktreePath: session.worktreePath,
      backend: session.backend, model: session.model, reasoningEffort: session.reasoningEffort, title: session.title,
      createdAt: session.createdAt, lastActiveAt: session.lastActiveAt, archivedAt: session.archivedAt,
      outcome: session.outcome, settledAt: session.settledAt,
      settledReason: session.settledReason
    };
    return request.action === 'prepare' ? { session: result } : result;
  }
  if (request.action === 'models' && object(value) && Array.isArray(value.models) && Array.isArray(value.reasoningEfforts)
    && nullableString(value.currentModelId) && nullableString(value.currentReasoningEffortId)) {
    const choices = (entries: unknown[]) => {
      const result = entries.map(entry => {
        if (!object(entry) || typeof entry.id !== 'string' || !entry.id || typeof entry.name !== 'string' || !optionalString(entry.description)) throw invalid();
        return { id: entry.id, name: entry.name, ...(entry.description === undefined ? {} : { description: entry.description }) };
      });
      if (new Set(result.map(entry => entry.id)).size !== result.length) throw invalid();
      return result;
    };
    const usage = value.contextUsage;
    let contextUsage: { size: number; used: number } | null = null;
    if (usage !== null) {
      if (!object(usage) || !timestamp(usage.size) || !timestamp(usage.used)) throw invalid();
      contextUsage = { size: usage.size, used: usage.used };
    }
    return { models: choices(value.models), reasoningEfforts: choices(value.reasoningEfforts),
      currentModelId: value.currentModelId, currentReasoningEffortId: value.currentReasoningEffortId,
      contextUsage };
  }
  if (request.action === 'commands' && Array.isArray(value)) {
    return value.map(entry => {
      if (!object(entry) || typeof entry.name !== 'string' || !entry.name || typeof entry.description !== 'string' || !optionalString(entry.argsHint)) throw invalid();
      return { name: entry.name, description: entry.description, ...(entry.argsHint === undefined ? {} : { argsHint: entry.argsHint }) };
    });
  }
  if ((request.action === 'cancel' || request.action === 'steer' || request.action === 'permission') && object(value) && value.success === true) return { success: true };
  throw invalid();
}

/** Bound decoded response bytes even when a worker omits or lies about Content-Length. */
export async function readWorkerChatReply(response: Response, request: Record<string, unknown>): Promise<WorkerChatReply> {
  if (!response.headers.get('content-type')?.startsWith('application/json') || !response.body) {
    await response.body?.cancel();
    throw new Error('Worker returned an invalid chat response.');
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let size = 0;
  let text = '';
  try {
    while (true) {
      const part = await reader.read();
      size += part.value?.byteLength ?? 0;
      if (size > 2_000_000) throw new Error('Worker chat response exceeds the supported size.');
      text += decoder.decode(part.value, { stream: !part.done });
      if (part.done) {
        let value: unknown;
        try { value = JSON.parse(text); }
        catch { throw new Error('Worker returned an invalid chat response.'); }
        return parseWorkerChatReply(value, request);
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
}
