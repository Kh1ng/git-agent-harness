import { appendFileSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type {
  ChatUsage,
  GitReviewState,
  HelperRoutePreference,
  HelperSuggestion,
  HelperTaskKind,
  HelperUsageRecord
} from '@git-agent-harness/contracts';
import { isUsageLimitError } from './acpAdapter.js';
import { stateBase } from './chatSessions.js';
import { resolveInstanceAdapter, type ManagerAdapter } from './registry.js';
import { helperRouteFor } from './settingsStore.js';

const CHAT_TITLE_INPUT_LIMIT = 800;
const DIFF_INPUT_LIMIT = 48_000;
const OUTPUT_LIMIT = 12_000;
const HELPER_TIMEOUT_MS = 30_000;

export interface HelperTaskResult extends HelperSuggestion {
  usage: ChatUsage | null;
  latencyMs: number;
}

interface HelperTaskRequest {
  profile: string;
  sourceBackend: string;
  sourceBackendInstance: string | null;
  kind: HelperTaskKind;
  input: string;
  fallback: Pick<HelperSuggestion, 'text' | 'title' | 'body'>;
}

interface HelperTaskDeps {
  preference?: HelperRoutePreference;
  adapter?: (profile: string, backend: string, instance: string | null) => Promise<ManagerAdapter>;
  timeoutMs?: number;
  now?: () => number;
}

const queues = new Map<string, Promise<void>>();

function bounded(value: string, limit: number): string {
  return value.replace(/\0/g, '').slice(0, limit);
}

export function chatTitleInput(message: string): string {
  return bounded(message, CHAT_TITLE_INPUT_LIMIT);
}

export function commitMessageInput(files: string[], patch: string): string {
  const names = files.slice(0, 100).map(file => bounded(file, 300));
  return `Selected files:\n${names.map(file => `- ${file}`).join('\n')}\n\nSelected diff:\n${bounded(patch, DIFF_INPUT_LIMIT)}`;
}

export function prSummaryInput(review: Pick<GitReviewState, 'branch' | 'base' | 'commits' | 'patch'>): string {
  const subjects = review.commits.slice(0, 100).map(commit => bounded(commit.subject, 500));
  const references = [...new Set([review.branch, ...subjects].flatMap(value => value.match(/#\d+/g) ?? []))].slice(0, 20);
  return `Base: ${bounded(review.base, 200)}\nHead: ${bounded(review.branch, 200)}\nCommits:\n${subjects.map(subject => `- ${subject}`).join('\n') || '- None'}\nLinked issue references: ${references.join(', ') || 'None'}\n\nCommitted diff:\n${bounded(review.patch, DIFF_INPUT_LIMIT)}`;
}

function prompt(kind: HelperTaskKind, input: string): string {
  if (kind === 'chat_title') return `Write a concise chat title of at most 64 characters. Return only the title.\n\n${input}`;
  if (kind === 'commit_message') return `Write one concise imperative git commit subject of at most 120 characters. Return only the subject.\n\n${input}`;
  return `Write editable pull request prose from only the supplied committed changes. Return exactly:\nTITLE: one concise title\nBODY:\nmarkdown body\n\n${input}`;
}

function parseReply(kind: HelperTaskKind, reply: string): Pick<HelperSuggestion, 'text' | 'title' | 'body'> | null {
  const clean = bounded(reply.trim(), OUTPUT_LIMIT);
  if (!clean) return null;
  if (kind === 'pr_summary') {
    const match = /^TITLE:\s*([^\r\n]+)\r?\nBODY:\s*\r?\n?([\s\S]*)$/i.exec(clean);
    if (!match) return null;
    const title = match[1].trim().slice(0, 200);
    const body = match[2].trim().slice(0, OUTPUT_LIMIT);
    return title ? { text: body, title, body } : null;
  }
  const text = clean.split(/\r?\n/)[0].replace(/^['"]|['"]$/g, '').trim().slice(0, kind === 'chat_title' ? 64 : 120);
  return text ? { text } : null;
}

function fallback(request: HelperTaskRequest, reason: string, startedAt: number, now: () => number): HelperTaskResult {
  return helperFallback(request.kind, request.fallback, reason, Math.max(0, now() - startedAt));
}

export function helperFallback(
  kind: HelperTaskKind,
  value: Pick<HelperSuggestion, 'text' | 'title' | 'body'>,
  reason: string,
  latencyMs = 0
): HelperTaskResult {
  return {
    kind,
    ...value,
    generated: false,
    backend: null,
    backendInstance: null,
    model: null,
    fallbackReason: reason,
    usage: null,
    latencyMs
  };
}

function lunaModel(models: { id: string; name: string }[]): string | null {
  return models.find(model => /luna/i.test(`${model.id} ${model.name}`))?.id ?? null;
}

function failureReason(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  if (/timeout/i.test(text)) return 'timeout';
  if (isUsageLimitError(error)) return 'quota_exhausted';
  if (/auth|login|credential/i.test(text)) return 'missing_auth';
  if (/model/i.test(text)) return 'missing_model';
  return 'helper_unavailable';
}

async function withTimeout<T>(operation: Promise<T>, timeoutMs: number, onTimeout: () => void): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<T>((_, reject) => {
        timer = setTimeout(() => {
          onTimeout();
          reject(new Error('Helper timeout'));
        }, timeoutMs);
        timer.unref?.();
      })
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function run(request: HelperTaskRequest, deps: HelperTaskDeps): Promise<HelperTaskResult> {
  const now = deps.now ?? Date.now;
  const startedAt = now();
  const preference = deps.preference ?? helperRouteFor(request.profile, request.sourceBackend, request.sourceBackendInstance);
  if (preference?.enabled === false) return fallback(request, 'disabled', startedAt, now);
  const backend = preference?.backend ?? request.sourceBackend;
  const backendInstance = preference ? preference.backendInstance : request.sourceBackendInstance;
  if (backend !== 'codex' && !preference?.model) return fallback(request, 'missing_model', startedAt, now);
  const adapterFor = deps.adapter ?? ((profile, targetBackend, instance) => resolveInstanceAdapter(profile, targetBackend, instance));
  const key = `helper:${request.profile}:${backend}:${backendInstance ?? 'default'}`;
  try {
    const adapter = await adapterFor(request.profile, backend, backendInstance);
    const catalog = await withTimeout(adapter.listModels(key), deps.timeoutMs ?? HELPER_TIMEOUT_MS, () => void adapter.cancelTurn(key));
    const model = preference?.model ?? (backend === 'codex' ? lunaModel(catalog.models) : null);
    if (!model || !catalog.models.some(candidate => candidate.id === model)) {
      return fallback(request, 'missing_model', startedAt, now);
    }
    const result = await withTimeout(adapter.runTurn(key, {
      prompt: prompt(request.kind, request.input),
      history: [],
      model,
      onChunk: () => {},
      onToolResult: () => {}
    }), deps.timeoutMs ?? HELPER_TIMEOUT_MS, () => void adapter.cancelTurn(key));
    const parsed = parseReply(request.kind, result.reply);
    if (!parsed) return fallback(request, 'invalid_output', startedAt, now);
    return {
      kind: request.kind,
      ...parsed,
      generated: true,
      backend,
      backendInstance,
      model: result.model ?? model,
      fallbackReason: null,
      usage: result.usage,
      latencyMs: Math.max(0, now() - startedAt)
    };
  } catch (error) {
    return fallback(request, failureReason(error), startedAt, now);
  }
}

export async function runHelperTask(request: HelperTaskRequest, deps: HelperTaskDeps = {}): Promise<HelperTaskResult> {
  const preference = deps.preference ?? helperRouteFor(request.profile, request.sourceBackend, request.sourceBackendInstance);
  const route = `${request.profile}\0${preference?.backend ?? request.sourceBackend}\0${preference ? preference.backendInstance ?? '' : request.sourceBackendInstance ?? ''}`;
  const prior = queues.get(route) ?? Promise.resolve();
  let release!: () => void;
  const next = new Promise<void>(resolve => { release = resolve; });
  const queued = prior.then(() => next);
  queues.set(route, queued);
  await prior;
  try {
    return await run(request, preference ? { ...deps, preference } : deps);
  } finally {
    release();
    if (queues.get(route) === queued) queues.delete(route);
  }
}

function usagePath(): string {
  return join(stateBase(), 'helper-usage.jsonl');
}

export function recordHelperUsage(profile: string, result: HelperTaskResult): void {
  const record: HelperUsageRecord = {
    timestamp: Date.now(),
    kind: result.kind,
    profile,
    backend: result.backend,
    backendInstance: result.backendInstance,
    model: result.model,
    inputTokens: result.usage?.input_tokens ?? null,
    outputTokens: result.usage?.output_tokens ?? null,
    totalTokens: result.usage?.total_tokens ?? null,
    latencyMs: result.latencyMs,
    fallbackReason: result.fallbackReason
  };
  const path = usagePath();
  mkdirSync(dirname(path), { recursive: true });
  appendFileSync(path, `${JSON.stringify(record)}\n`, { mode: 0o600 });
}

export function readHelperUsage(limit = 100): HelperUsageRecord[] {
  try {
    return readFileSync(usagePath(), 'utf8').trim().split('\n').filter(Boolean).slice(-Math.min(500, Math.max(1, limit)))
      .map(line => JSON.parse(line) as HelperUsageRecord);
  } catch {
    return [];
  }
}

export function publicSuggestion(result: HelperTaskResult): HelperSuggestion {
  const { usage: _usage, latencyMs: _latencyMs, ...suggestion } = result;
  return suggestion;
}
