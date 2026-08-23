/**
 * Event-sourced chat session log (issue #955).
 *
 * A project chat is an append-only log of typed session events; the transcript
 * the UI renders is DERIVED from the log (a fold), never stored separately.
 * The store appends events to a per-profile JSONL file, reloads it on request,
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
  writeFileSync
} from 'node:fs';
import { dirname, resolve } from 'node:path';
import type {
  ChatSessionEvent,
  ChatSessionView,
  ChatTranscriptTurn,
  ChatTurnEnd
} from '@git-agent-harness/contracts';

export interface SessionLogOptions {
  /** Override the state directory (tests). Default: $XDG_STATE_HOME/gah/chat, else ~/.local/state/gah/chat. */
  stateDir?: string;
}

/** A parsed event with its file line number, for exact truncation. */
interface LoggedEvent {
  line: number;
  event: ChatSessionEvent;
}

/** Where the per-profile session log lives. */
export function chatLogPath(profile: string, opts: SessionLogOptions = {}): string {
  const base =
    opts.stateDir ??
    process.env.GAH_CHAT_STATE_DIR ??
    (process.env.XDG_STATE_HOME
      ? resolve(process.env.XDG_STATE_HOME, 'gah', 'chat')
      : (process.env.HOME
          ? resolve(process.env.HOME, '.local', 'state', 'gah', 'chat')
          : resolve(process.cwd(), 'config', 'chat')));
  return resolve(base, `${profile.replace(/[^a-zA-Z0-9_-]/g, '_')}.jsonl`);
}

function parseEventLine(line: string, lineNo: number): ChatSessionEvent | null {
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
export function readLog(profile: string, opts: SessionLogOptions = {}): LoggedEvent[] {
  const path = chatLogPath(profile, opts);
  if (!existsSync(path)) return [];
  const text = readFileSync(path, 'utf8');
  const out: LoggedEvent[] = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const event = parseEventLine(lines[i], i + 1);
    if (event) out.push({ line: i + 1, event });
  }
  return out;
}

/** Append a batch of events to the log. */
export function appendEvents(profile: string, events: ChatSessionEvent[], opts: SessionLogOptions = {}): void {
  if (events.length === 0) return;
  const path = chatLogPath(profile, opts);
  mkdirSync(dirname(path), { recursive: true });
  const payload = events.map((e) => `${JSON.stringify(e)}\n`).join('');
  appendFileSync(path, payload, 'utf8');
}

/** Rewrite the log to exactly the given events (used for repair). */
function rewriteLog(profile: string, events: ChatSessionEvent[], opts: SessionLogOptions = {}): void {
  const path = chatLogPath(profile, opts);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, events.map((e) => `${JSON.stringify(e)}\n`).join(''), 'utf8');
}
/**
 * Load + repair: read the log, and for any turn left open by a crash append a
 * synthetic turn/end { kind: 'interrupted' }. Returns the durable event list.
 */
export function loadLog(profile: string, opts: SessionLogOptions = {}): ChatSessionEvent[] {
  const logged = readLog(profile, opts);
  const events = logged.map((l) => l.event);

  // Find open turns: a turn/start whose matching turn/end never arrived.
  const openTurns = new Set<number>();
  for (const e of events) {
    if (e.type === 'turn/start') openTurns.add(e.turn);
    else if (e.type === 'turn/end') openTurns.delete(e.turn);
  }

  if (openTurns.size === 0) return events;

  // Close each open turn with a synthetic interrupted end, then rewrite.
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
  rewriteLog(profile, repaired, opts);
  return repaired;
}

/**
 * Fold the log into the derived transcript the UI renders.
 * `cursor` is the highest seq; pass `sinceSeq` to resume streaming from a
 * position (the fold still computes the full transcript; the caller can slice).
 */
export function foldSession(profile: string, opts: SessionLogOptions = {}): ChatSessionView {
  const events = loadLog(profile, opts);
  const turns: ChatTranscriptTurn[] = [];
  let streaming: ChatSessionView['streaming'] = null;
  let cursor = 0;
  let partialText = '';
  let openTurn: number | null = null;

  for (const e of events) {
    if (e.seq > cursor) cursor = e.seq;
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
        turns.push({ role: 'user', text: e.text, timestamp: e.timestamp });
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
      case 'human/command':
        turns.push({ role: 'system', text: `/${e.command} ${e.result}`.trim(), timestamp: e.timestamp });
        break;
      case 'compaction/summary':
        turns.push({ role: 'system', text: `Compacted: ${e.summary}`, timestamp: e.timestamp });
        break;
    }
  }

  if (openTurn !== null && partialText) {
    streaming = { turn: openTurn, partialText };
  }

  return { profile, cursor, turns, streaming };
}
