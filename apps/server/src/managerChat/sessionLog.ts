/**
 * Event-sourced chat session log (issue #955).
 *
 * A project chat is an append-only log of typed session events; the transcript
 * the UI renders is DERIVED from the log (a fold), never stored separately.
 * The store appends events to a JSONL file per project and session, reloads it on request,
 * and repairs an interrupted turn (turn/start with no matching turn/end) with
 * a synthetic turn/end { kind: 'interrupted' } rather than truncating.
 *
 * This replaces the in-memory `historyByProfile` array in ManagerChatManager:
 * the log survives server restart, resume derives from the same stream, and
 * per-message backend/model/usage attribution is carried on each event.
 */

import {
  mkdirSync,
  readFileSync,
  appendFileSync,
  existsSync,
  createWriteStream
} from 'node:fs';
import { dirname, resolve } from 'node:path';
import type {
  ChatSessionEvent,
  ChatSessionView,
  ChatTranscriptTurn,
  ChatTurnEnd,
  ChatUserMessage
} from '@git-agent-harness/contracts';

export interface SessionLogOptions {
  /** Override the state directory (tests). Default: $XDG_STATE_HOME/gah/chat, else ~/.local/state/gah/chat. */
  stateDir?: string;
  /** Stable session identity within the project. */
  sessionId?: string;
}

/** Where one project session log lives. The profile is the current project key. */
export function chatLogPath(profile: string, opts: SessionLogOptions = {}): string {
  const base =
    opts.stateDir ??
    process.env.GAH_CHAT_STATE_DIR ??
    (process.env.XDG_STATE_HOME
      ? resolve(process.env.XDG_STATE_HOME, 'gah', 'chat')
      : (process.env.HOME
          ? resolve(process.env.HOME, '.local', 'state', 'gah', 'chat')
          : resolve(process.cwd(), 'config', 'chat')));
  return resolve(
    base,
    `project-${encodeURIComponent(profile)}`,
    `session-${encodeURIComponent(opts.sessionId ?? 'default')}`,
    'session.jsonl'
  );
}

function parseEventLine(line: string): ChatSessionEvent | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  try {
    const parsed = JSON.parse(trimmed) as ChatSessionEvent;
    if (!parsed || typeof parsed !== 'object' || typeof parsed.type !== 'string') return null;
    return parsed;
  } catch {
    return null;
  }
}

/** Read the full log, skipping malformed lines. Returns events in order. */
export function readLog(profile: string, opts: SessionLogOptions = {}): ChatSessionEvent[] {
  const path = chatLogPath(profile, opts);
  if (!existsSync(path)) return [];
  const text = readFileSync(path, 'utf8');
  const events: ChatSessionEvent[] = [];
  for (const line of text.split('\n')) {
    const event = parseEventLine(line);
    if (event) events.push(event);
  }
  return events;
}

/** Append a batch of events to the log. */
export function appendEvents(profile: string, events: ChatSessionEvent[], opts: SessionLogOptions = {}): void {
  if (events.length === 0) return;
  const path = chatLogPath(profile, opts);
  mkdirSync(dirname(path), { recursive: true });
  const payload = events.map((e) => `${JSON.stringify(e)}\n`).join('');
  appendFileSync(path, payload, 'utf8');
}

/** Stream high-frequency events without blocking Node's event loop per chunk. */
export function createEventWriter(profile: string, opts: SessionLogOptions = {}) {
  const path = chatLogPath(profile, opts);
  mkdirSync(dirname(path), { recursive: true });
  const stream = createWriteStream(path, { flags: 'a', encoding: 'utf8' });
  let failure: Error | undefined;
  let closePromise: Promise<void> | undefined;
  stream.on('error', (error) => { failure = error; });

  return {
    append(event: ChatSessionEvent): void {
      if (failure) throw failure;
      stream.write(`${JSON.stringify(event)}\n`);
    },
    close(): Promise<void> {
      if (failure) return Promise.reject(failure);
      closePromise ??= new Promise<void>((resolve, reject) => {
        stream.once('error', reject);
        stream.end(() => failure ? reject(failure) : resolve());
      });
      return closePromise;
    }
  };
}

/**
 * Load + repair: read the log, and for any turn left open by a crash append a
 * synthetic turn/end { kind: 'interrupted' }. Returns the durable event list.
 */
export function loadLog(
  profile: string,
  opts: SessionLogOptions = {},
  repairInterrupted = true
): ChatSessionEvent[] {
  const events = readLog(profile, opts);

  if (!repairInterrupted) return events;

  // Find open turns: a turn/start whose matching turn/end never arrived.
  const openTurns = new Set<number>();
  for (const e of events) {
    if (e.type === 'turn/start') openTurns.add(e.turn);
    else if (e.type === 'turn/end') openTurns.delete(e.turn);
  }

  if (openTurns.size === 0) return events;

  // Close each open turn by appending synthetic interrupted ends.
  const repaired: ChatSessionEvent[] = [...events];
  let seq = repaired.length ? Math.max(...repaired.map((e) => e.seq)) : 0;
  for (const turn of [...openTurns].sort((a, b) => a - b)) {
    seq += 1;
    const end: ChatTurnEnd = {
      type: 'turn/end',
      seq,
      turn,
      reason: { kind: 'interrupted' },
      timestamp: Date.now()
    };
    repaired.push(end);
  }
  const path = chatLogPath(profile, opts);
  if (!readFileSync(path, 'utf8').endsWith('\n')) appendFileSync(path, '\n', 'utf8');
  appendEvents(profile, repaired.slice(events.length), opts);
  return repaired;
}

/** Derive the exact user and assistant messages used to resume a model. */
export function deriveModelHistory(events: ChatSessionEvent[]): ChatTranscriptTurn[] {
  events = events.slice(completedCompactionBoundary(events));
  const prompts = new Map<number, ChatUserMessage>();
  for (const event of events) {
    if (event.type === 'user/message' && (event.source === 'inject' || !prompts.has(event.turn))) {
      prompts.set(event.turn, event);
    }
  }

  const history: ChatTranscriptTurn[] = [];
  for (const [index, event] of events.entries()) {
    if (event.type === 'user/message' && event.source === 'prompt') {
      const prompt = prompts.get(event.turn) ?? event;
      history.push({ role: 'user', text: prompt.text, timestamp: prompt.timestamp });
    } else if (event.type === 'assistant/message') {
      history.push({
        role: 'assistant',
        text: event.text,
        timestamp: event.timestamp,
        backend: event.backend,
        model: event.model,
        usage: event.usage
      });
    } else if (event.type === 'tool/result' && !isLegacyHarnessError(events, index)) {
      history.push({ role: 'system', text: `[${event.name}] ${event.text}`, timestamp: event.timestamp });
    } else if (event.type === 'compaction/summary') {
      history.push({ role: 'system', text: event.summary, timestamp: event.timestamp });
    }
  }
  return history;
}

function isLegacyHarnessError(events: ChatSessionEvent[], index: number): boolean {
  const event = events[index];
  const end = events[index + 1];
  return event.type === 'tool/result' && event.name === 'error' &&
    end?.type === 'turn/end' && end.turn === event.turn &&
    end.reason.kind === 'error' && end.reason.message === event.text;
}

function completedCompactionBoundary(events: ChatSessionEvent[]): number {
  let end = -1;
  for (let index = events.length - 1; index >= 0; index--) {
    if (events[index].type === 'compaction/end') {
      end = index;
      break;
    }
  }
  if (end < 0) return 0;
  const turn = events[end].turn;
  for (let index = end; index >= 0; index--) {
    const event = events[index];
    if (event.type === 'compaction/summary' && event.turn === turn) return index;
  }
  // Old logs predate required summaries. Preserve their former boundary.
  for (let index = end; index >= 0; index--) {
    const event = events[index];
    if (event.type === 'compaction/start' && event.turn === turn) return index;
  }
  return 0;
}

/**
 * Fold the log into the derived transcript the UI renders.
 * `cursor` is the highest seq; pass `sinceSeq` to resume streaming from a
 * position (the fold still computes the full transcript; the caller can slice).
 */
export function foldSession(
  profile: string,
  opts: SessionLogOptions = {},
  repairInterrupted = true
): ChatSessionView {
  const events = loadLog(profile, opts, repairInterrupted);
  const turns: ChatTranscriptTurn[] = [];
  let streaming: ChatSessionView['streaming'] = null;
  let cursor = events.reduce((highest, event) => Math.max(highest, event.seq), 0);
  let partialText = '';
  let openTurn: number | null = null;

  for (const e of events.slice(completedCompactionBoundary(events))) {
    switch (e.type) {
      case 'turn/start':
        openTurn = e.turn;
        partialText = '';
        break;
      case 'turn/end':
        if (openTurn === e.turn) {
          if (e.reason.kind === 'interrupted' && partialText) {
            turns.push({ role: 'assistant', text: partialText, timestamp: e.timestamp });
          }
          openTurn = null;
          partialText = '';
        }
        break;
      case 'user/message':
        if (e.source === 'prompt') {
          turns.push({ role: 'user', text: e.text, timestamp: e.timestamp });
        }
        break;
      case 'assistant/chunk':
        partialText += e.text;
        break;
      case 'assistant/message':
        turns.push({
          role: 'assistant',
          text: e.text,
          timestamp: e.timestamp,
          backend: e.backend,
          model: e.model,
          usage: e.usage
        });
        partialText = '';
        break;
      case 'tool/result':
        turns.push({ role: 'system', text: `[${e.name}] ${e.text}`, timestamp: e.timestamp });
        break;
      case 'harness/error':
        turns.push({ role: 'system', text: `[error] ${e.text}`, timestamp: e.timestamp });
        break;
      case 'human/command':
        if (turns.at(-1)?.role !== 'assistant' || turns.at(-1)?.text !== e.result) {
          turns.push({ role: 'system', text: `/${e.command} ${e.result}`.trim(), timestamp: e.timestamp });
        }
        break;
      case 'compaction/start':
      case 'compaction/end':
        break;
      case 'compaction/summary':
        turns.push({ role: 'system', text: `Compacted: ${e.summary}`, timestamp: e.timestamp });
        break;
    }
  }

  if (openTurn !== null) {
    streaming = { turn: openTurn, partialText };
  }

  return { profile, cursor, turns, streaming };
}
