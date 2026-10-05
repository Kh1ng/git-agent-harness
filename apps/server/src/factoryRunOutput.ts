import { closeSync, openSync, readdirSync, readFileSync, readlinkSync, readSync, statSync } from 'node:fs';
import { basename } from 'node:path';
import type { FactoryRunEvent, FactoryRunOutput } from '@git-agent-harness/contracts';

const RUN_LOG = /\/sessions\/([0-9a-f-]{36})\/attempt-(\d+)\/backend-output\.log$/i;
const RUN_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
/** How much of a log one read returns, and how much of a command's output. */
const READ_BYTES = 256 * 1024;
const OUTPUT_TAIL = 4000;
/** The longest single line the reader will widen its window for. */
const MAX_LINE_BYTES = 8 * 1024 * 1024;
const TEXT_LIMIT = 4000;

/** The backend output logs that running `gah` processes hold open, by run id
 * (the newest attempt wins). The loop keeps a job's log open while its agent
 * runs, so this needs no knowledge of where a profile keeps its artifacts. */
export function openRunLogs(): Map<string, { file: string; attempt: number }> {
  const logs = new Map<string, { file: string; attempt: number }>();
  if (process.platform !== 'linux') return logs;
  for (const entry of readdirSync('/proc')) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const argv0 = readFileSync(`/proc/${entry}/cmdline`, 'utf8').split('\0')[0] ?? '';
      if (basename(argv0) !== 'gah') continue;
      for (const fd of readdirSync(`/proc/${entry}/fd`)) {
        let target: string;
        try { target = readlinkSync(`/proc/${entry}/fd/${fd}`); } catch { continue; }
        const match = RUN_LOG.exec(target);
        if (!match) continue;
        const attempt = Number(match[2]);
        const known = logs.get(match[1]);
        if (!known || attempt > known.attempt) logs.set(match[1], { file: target, attempt });
      }
    } catch {
      // Not our process, or it exited mid-read.
    }
  }
  return logs;
}

const clip = (text: string, limit = TEXT_LIMIT) => (text.length > limit ? `${text.slice(0, limit)}…` : text);
const tail = (text: string) => (text.length > OUTPUT_TAIL ? `…${text.slice(-OUTPUT_TAIL)}` : text);

/** Turn one log line into what a person watching wants to see. Understands
 * Codex `exec --json` items and Claude `stream-json` messages; anything else
 * is passed through as a raw line. A step's "started" echo is dropped once
 * its completion is in the same batch. */
export function parseRunLine(line: string): FactoryRunEvent[] {
  const trimmed = line.trim();
  if (!trimmed) return [];
  let record: Record<string, unknown>;
  try { record = JSON.parse(trimmed) as Record<string, unknown>; } catch { return [{ kind: 'raw', text: clip(trimmed, 300) }]; }
  const type = typeof record.type === 'string' ? record.type : '';
  const item = record.item as Record<string, unknown> | undefined;
  if (item && (type === 'item.started' || type === 'item.completed' || type === 'item.updated')) {
    const done = type === 'item.completed';
    if (item.type === 'agent_message' || item.type === 'reasoning') return typeof item.text === 'string' && done ? [{ kind: 'message', text: clip(item.text) }] : [];
    if (item.type === 'command_execution') {
      const exit = typeof item.exit_code === 'number' ? item.exit_code : null;
      return [{
        kind: 'command', text: clip(String(item.command ?? ''), 1000),
        output: typeof item.aggregated_output === 'string' && item.aggregated_output ? tail(item.aggregated_output) : null,
        status: !done ? 'running' : exit !== null && exit !== 0 ? 'failed' : 'completed', exit_code: exit
      }];
    }
    if (item.type === 'file_change') {
      const changes = Array.isArray(item.changes) ? item.changes as { path?: unknown; kind?: unknown }[] : [];
      return done ? [{ kind: 'file_change', text: changes.map((change) => `${String(change.kind ?? 'update')} ${String(change.path ?? '')}`).join('\n'), status: 'completed' }] : [];
    }
    return done ? [{ kind: 'raw', text: clip(`${String(item.type)}: ${JSON.stringify(item)}`, 500) }] : [];
  }
  if (type === 'assistant' || type === 'user') {
    const content = (record.message as { content?: unknown } | undefined)?.content;
    if (!Array.isArray(content)) return [];
    return content.flatMap((part: Record<string, unknown>): FactoryRunEvent[] => {
      if (type === 'assistant' && part.type === 'text' && typeof part.text === 'string') return [{ kind: 'message', text: clip(part.text) }];
      if (type === 'assistant' && part.type === 'tool_use') return [{ kind: 'command', text: clip(`${String(part.name)} ${JSON.stringify(part.input ?? {})}`, 1000), status: 'running' }];
      if (type === 'user' && part.type === 'tool_result') {
        const body = typeof part.content === 'string' ? part.content : JSON.stringify(part.content ?? '');
        return [{ kind: 'command', text: 'tool result', output: tail(body), status: part.is_error ? 'failed' : 'completed' }];
      }
      return [];
    });
  }
  if (type === 'thread.started' || type === 'turn.started' || type === 'turn.completed' || type === 'system' || type === 'result') {
    return type === 'result' && typeof record.result === 'string' ? [{ kind: 'message', text: clip(record.result) }] : [];
  }
  return [{ kind: 'raw', text: clip(trimmed, 500) }];
}

/** Read a run's log from `after`, whole lines only. The first read of a long
 * log starts near its end. A command's "running" event is dropped when the
 * same read also holds its result. */
export function readRunOutput(file: string, attempt: number, after: number): FactoryRunOutput {
  const size = statSync(file).size;
  let start = Number.isFinite(after) && after > 0 && after <= size ? Math.floor(after) : 0;
  const truncated = start === 0 && size > READ_BYTES;
  if (truncated) start = size - READ_BYTES;
  // One line can outgrow the window (a command's whole output is on it):
  // widen the read until it holds a complete line, up to a hard limit.
  let window = READ_BYTES;
  let text = '';
  for (;;) {
    const length = Math.min(size - start, window);
    const buffer = Buffer.alloc(length);
    const fd = openSync(file, 'r');
    let read = 0;
    try { read = readSync(fd, buffer, 0, length, start); } finally { closeSync(fd); }
    text = buffer.toString('utf8', 0, read);
    if (truncated && window === READ_BYTES) {
      // Skip the partial first line the window cut into.
      const firstBreak = text.indexOf('\n');
      if (firstBreak !== -1) {
        start += Buffer.byteLength(text.slice(0, firstBreak + 1));
        text = text.slice(firstBreak + 1);
      }
    }
    if (text.includes('\n') || start + length >= size || window >= MAX_LINE_BYTES) break;
    window *= 2;
  }
  if (!text.includes('\n') && window >= MAX_LINE_BYTES && start + window < size) {
    // A line longer than the limit: step over what was read so the view keeps moving.
    return { found: true, attempt, truncated: true, next: start + Buffer.byteLength(text), events: [{ kind: 'raw', text: 'A very long output line was skipped.' }] };
  }
  const lastBreak = text.lastIndexOf('\n');
  const complete = lastBreak === -1 ? '' : text.slice(0, lastBreak + 1);
  const events = complete.split('\n').flatMap(parseRunLine);
  const finished = new Set(events.filter((event) => event.kind === 'command' && event.status !== 'running').map((event) => event.text));
  return {
    found: true, attempt, truncated,
    next: start + Buffer.byteLength(complete),
    events: events.filter((event) => !(event.kind === 'command' && event.status === 'running' && finished.has(event.text)))
  };
}

/** The output of a run a loop on this node is working on right now. */
export function factoryRunOutput(runId: string, after: number, logs = openRunLogs()): FactoryRunOutput {
  const log = RUN_ID.test(runId) ? logs.get(runId) : undefined;
  if (!log) return { found: false, attempt: null, next: 0, truncated: false, events: [] };
  return readRunOutput(log.file, log.attempt, after);
}
