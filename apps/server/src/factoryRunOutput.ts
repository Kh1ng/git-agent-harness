import { closeSync, openSync, readdirSync, readFileSync, readlinkSync, readSync, statSync } from 'node:fs';
import { basename, join, resolve, sep } from 'node:path';
import type { FactoryRunEvent, FactoryRunOutput } from '@git-agent-harness/contracts';

/** A run's agent output: an implementation attempt's `backend-output.log`,
 * or a review attempt's `review-stdout.log`. */
const RUN_LOG = /^(.*\/sessions)\/([0-9a-f-]{36})\/(?:(?:review-)?attempt-(\d+)\/)?(?:backend-output|review-stdout)\.log$/i;
const RUN_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
/** How much of a log one read returns, and how much of a command's output. */
const READ_BYTES = 256 * 1024;
const OUTPUT_TAIL = 4000;
/** The longest single line the reader will widen its window for. */
const MAX_LINE_BYTES = 8 * 1024 * 1024;
const TEXT_LIMIT = 4000;

export interface RunLog { file: string; attempt: number }

/** `sessions` directories seen under a running loop, kept so a job that has
 * just ended (and whose log the loop has closed) can still be read. */
const knownSessionRoots = new Set<string>();

/** The agent output logs that running `gah` processes hold open, by run id
 * (the newest wins), plus the `sessions` directories they live in. The loop
 * keeps a job's log open while its agent runs, so this needs no knowledge of
 * where a profile keeps its artifacts. */
export function openRunLogs(): { logs: Map<string, RunLog>; roots: Set<string> } {
  const logs = new Map<string, RunLog>();
  if (process.platform !== 'linux') return { logs, roots: knownSessionRoots };
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
        knownSessionRoots.add(match[1]);
        const attempt = Number(match[3] ?? 1);
        const known = logs.get(match[2]);
        // Within a run, the log written last is the step in progress (a review follows its attempt).
        if (!known || mtime(target) >= mtime(known.file)) logs.set(match[2], { file: target, attempt });
      }
    } catch {
      // Not our process, or it exited mid-read.
    }
  }
  return { logs, roots: knownSessionRoots };
}

function mtime(file: string): number {
  try { return statSync(file).mtimeMs; } catch { return 0; }
}

/** A run's newest output log on disk, under one of the known `sessions`
 * directories: for a job whose log the loop has already closed. */
export function findRunLogOnDisk(runId: string, roots: Iterable<string>): RunLog | null {
  let best: (RunLog & { at: number }) | null = null;
  for (const root of roots) {
    const directory = resolve(join(root, runId));
    if (!directory.startsWith(root.endsWith(sep) ? root : root + sep)) continue;
    let entries: string[];
    try { entries = readdirSync(directory); } catch { continue; }
    for (const entry of ['', ...entries]) {
      const attempt = /^(?:review-)?attempt-(\d+)$/.exec(entry);
      if (entry && !attempt) continue;
      for (const name of ['backend-output.log', 'review-stdout.log']) {
        const file = join(directory, entry, name);
        const at = mtime(file);
        if (at > 0 && (!best || at > best.at)) best = { file, attempt: Number(attempt?.[1] ?? 1), at };
      }
    }
  }
  return best ? { file: best.file, attempt: best.attempt } : null;
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
  if (type === 'turn.failed' || type === 'error') {
    const message = (record.error as { message?: unknown } | undefined)?.message ?? record.message;
    return [{ kind: 'command', text: type === 'turn.failed' ? 'The agent\'s turn failed' : 'Error', output: typeof message === 'string' ? clip(message) : null, status: 'failed' }];
  }
  if (type === 'thread.started' || type === 'turn.started' || type === 'turn.completed' || type === 'system' || type === 'result') {
    return type === 'result' && typeof record.result === 'string' ? [{ kind: 'message', text: clip(record.result) }] : [];
  }
  return [{ kind: 'raw', text: clip(trimmed, 500) }];
}

/** Read a run's log from `after`, whole lines only. The first read of a long
 * log starts near its end. A command's "running" event is dropped when the
 * same read also holds its result. */
export function readRunOutput(file: string, attempt: number, after: number, full = false, log = basename(file)): FactoryRunOutput {
  const size = statSync(file).size;
  let start = Number.isFinite(after) && after > 0 && after <= size ? Math.floor(after) : 0;
  // `full` reads from the very start, for copying a job's whole output.
  const truncated = !full && start === 0 && size > READ_BYTES;
  if (truncated) start = size - READ_BYTES;
  // One line can outgrow the window (a command's whole output is on it):
  // widen the read until it holds a complete line, up to a hard limit.
  let window = READ_BYTES;
  let text = '';
  let read = 0;
  for (;;) {
    const length = Math.min(size - start, window);
    const buffer = Buffer.alloc(length);
    const fd = openSync(file, 'r');
    try { read = readSync(fd, buffer, 0, length, start); } finally { closeSync(fd); }
    let from = 0;
    if (truncated && window === READ_BYTES) {
      // Skip the partial first line the window cut into, in bytes: it may start mid-character.
      const firstBreak = buffer.subarray(0, read).indexOf(0x0a);
      if (firstBreak !== -1) from = firstBreak + 1;
    }
    start += from;
    read -= from;
    text = buffer.toString('utf8', from, from + read);
    if (text.includes('\n') || start + length >= size || window >= MAX_LINE_BYTES) break;
    window *= 2;
  }
  if (!text.includes('\n') && window >= MAX_LINE_BYTES && start + window < size) {
    // A line longer than the limit: step over what was read so the view keeps moving.
    return { found: true, attempt, log, truncated: true, next: start + read, events: [{ kind: 'raw', text: 'A very long output line was skipped.' }] };
  }
  const lastBreak = text.lastIndexOf('\n');
  const complete = lastBreak === -1 ? '' : text.slice(0, lastBreak + 1);
  const events = complete.split('\n').flatMap(parseRunLine);
  const finished = new Set(events.filter((event) => event.kind === 'command' && event.status !== 'running').map((event) => event.text));
  return {
    found: true, attempt, log, truncated,
    next: start + Buffer.byteLength(complete),
    events: events.filter((event) => !(event.kind === 'command' && event.status === 'running' && finished.has(event.text)))
  };
}

/** The output of a run a loop on this node is working on, or has just finished.
 * `after` is an offset into the log named `since`; when the run has moved on
 * to another log (a new attempt, or its review) reading starts over. */
export function factoryRunOutput(runId: string, after: number, open = openRunLogs(), full = false, since: string | null = null): FactoryRunOutput {
  if (!RUN_ID.test(runId)) return { found: false, attempt: null, log: null, next: 0, truncated: false, events: [] };
  const found = open.logs.get(runId) ?? findRunLogOnDisk(runId, open.roots);
  if (!found) return { found: false, attempt: null, log: null, next: 0, truncated: false, events: [] };
  // The log's path inside the run directory, e.g. `review-attempt-1/review-stdout.log`.
  const log = found.file.slice(found.file.lastIndexOf(runId) + runId.length + 1);
  return readRunOutput(found.file, found.attempt, since !== null && since !== log ? 0 : after, full, log);
}
