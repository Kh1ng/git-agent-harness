import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Check, Copy, FileEdit, Terminal } from 'lucide-react';
import type { FactoryRunEvent } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

const POLL_MS = 2000;
/** Keep the page light on a long job: older steps fall off the top. */
const MAX_EVENTS = 400;

/** A watchable job: the run behind a busy Factory Agents row. */
export interface WatchableRun { runId: string; title: string; subtitle: string | null }

/** A job's steps as plain text, for pasting into an issue or a chat. */
export function eventsAsText(events: FactoryRunEvent[]): string {
  return events.map((event) => {
    if (event.kind === 'command') {
      const status = event.status === 'failed' ? ` [failed${event.exit_code != null ? ` ${event.exit_code}` : ''}]` : event.status === 'running' ? ' [running]' : '';
      return `$ ${event.text}${status}${event.output ? `\n${event.output}` : ''}`;
    }
    return event.kind === 'file_change' ? `[files] ${event.text}` : event.text;
  }).join('\n\n');
}

function EventRow({ event }: { event: FactoryRunEvent }) {
  if (event.kind === 'message') return <li className="whitespace-pre-wrap break-words rounded-md bg-accent/10 px-3 py-2 text-sm text-primary">{event.text}</li>;
  if (event.kind === 'file_change') {
    return (
      <li className="flex items-start gap-2 px-1 text-xs text-secondary">
        <FileEdit size={13} className="mt-0.5 shrink-0 text-warning" aria-hidden="true" />
        <span className="whitespace-pre-wrap break-all font-mono">{event.text}</span>
      </li>
    );
  }
  if (event.kind === 'raw') return <li className="break-all px-1 font-mono text-[11px] text-muted">{event.text}</li>;
  const tone = event.status === 'failed' ? 'text-critical' : event.status === 'running' ? 'text-accent' : 'text-muted';
  return (
    <li className="rounded-md border border-subtle">
      <details open={event.status !== 'completed'}>
        <summary className="flex cursor-pointer list-none items-start gap-2 px-2 py-1.5 [&::-webkit-details-marker]:hidden">
          <Terminal size={13} className={`mt-0.5 shrink-0 ${tone}`} aria-hidden="true" />
          <span className="min-w-0 flex-1 break-all font-mono text-xs text-primary">{event.text}</span>
          <span className={`shrink-0 text-[11px] ${tone}`}>
            {event.status === 'running' ? 'running' : event.status === 'failed' ? `failed${event.exit_code != null ? ` (${event.exit_code})` : ''}` : 'done'}
          </span>
        </summary>
        {event.output && <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all border-t border-subtle bg-black/20 px-2 py-1.5 font-mono text-[11px] text-secondary">{event.output}</pre>}
      </details>
    </li>
  );
}

/**
 * View only: what a running factory agent is doing, as it does it. Follows
 * the job's backend output log by polling; nothing here can steer or stop
 * the job. Sticks to the newest step unless the reader has scrolled up.
 */
export function AgentLiveView({ runId, title, subtitle, onBack, runs = [], onSelectRun, alwaysListRuns = false }: {
  runId: string; title: string; subtitle?: string | null; onBack: () => void;
  /** Every factory agent running now, to switch between without leaving the view. */
  runs?: WatchableRun[];
  onSelectRun?: (run: WatchableRun) => void;
  /** Show the agent list even with a single agent (the Running agents sidebar). */
  alwaysListRuns?: boolean;
}) {
  const [copyState, setCopyState] = useState<'idle' | 'copying' | 'copied' | 'failed'>('idle');
  /** Copy the job's whole output: read the log from its start, not just what is on screen. */
  const copyAll = async () => {
    setCopyState('copying');
    try {
      const all: FactoryRunEvent[] = [];
      let log: string | null = null;
      for (let offset = 0, round = 0; round < 200; round++) {
        const output = await gahApi.getFactoryRunOutput(runId, offset, true, log);
        if (!output.found) break;
        // The run moved on to another log mid-copy: the server started it over, so does the copy.
        if (log !== null && output.log !== log) all.length = 0;
        log = output.log;
        const done = new Set(output.events.filter((event) => event.kind === 'command' && event.status !== 'running').map((event) => event.text));
        for (let index = all.length - 1; index >= 0; index--) if (all[index].kind === 'command' && all[index].status === 'running' && done.has(all[index].text)) all.splice(index, 1);
        all.push(...output.events);
        if (output.next === offset) break;
        offset = output.next;
      }
      await navigator.clipboard.writeText(`${title}\n\n${eventsAsText(all.length > 0 ? all : events)}`);
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
    window.setTimeout(() => setCopyState('idle'), 2000);
  };
  const [events, setEvents] = useState<FactoryRunEvent[]>([]);
  const [state, setState] = useState<'loading' | 'live' | 'gone' | 'error'>('loading');
  const [error, setError] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [follow, setFollow] = useState(true);
  /** Where the last read stopped: a byte offset into one of the run's logs. */
  const next = useRef<{ log: string | null; offset: number }>({ log: null, offset: 0 });
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let current = true;
    let timer: number | undefined;
    next.current = { log: null, offset: 0 };
    setEvents([]); setState('loading'); setTruncated(false);
    const poll = async () => {
      try {
        const output = await gahApi.getFactoryRunOutput(runId, next.current.offset, false, next.current.log);
        if (!current) return;
        if (!output.found) { setState((previous) => (previous === 'error' ? previous : 'gone')); return; }
        next.current = { log: output.log, offset: output.next };
        if (output.truncated) setTruncated(true);
        if (output.events.length > 0) {
          setEvents((previous) => {
            // A command that was running is replaced by its result when it arrives.
            const done = new Set(output.events.filter((event) => event.kind === 'command' && event.status !== 'running').map((event) => event.text));
            return [...previous.filter((event) => !(event.kind === 'command' && event.status === 'running' && done.has(event.text))), ...output.events].slice(-MAX_EVENTS);
          });
        }
        setState('live'); setError(null);
      } catch (err) {
        if (current) { setError(err instanceof Error ? err.message : String(err)); setState((previous) => (previous === 'loading' ? 'error' : previous)); }
      }
    };
    // The next poll waits for this one, so a slow response is never read twice.
    const loop = async () => {
      await poll();
      if (current) timer = window.setTimeout(loop, POLL_MS);
    };
    void loop();
    return () => { current = false; window.clearTimeout(timer); };
  }, [runId]);

  useEffect(() => { if (follow) end.current?.scrollIntoView({ block: 'end' }); }, [events, follow]);

  return (
    <section aria-label={`Live view of ${title}`} className="flex min-w-0 flex-col gap-3">
      {/* Pinned while the output scrolls: title, Copy all, the agent switcher and the follow toggle. */}
      <div className="sticky -top-4 z-10 -mx-4 flex flex-col gap-3 border-b border-subtle bg-page px-4 pb-2 pt-4" data-testid="live-view-header">
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-[11px] font-semibold uppercase tracking-wide text-muted">Live view · read only</p>
          <h2 className="mt-0.5 break-words text-base font-semibold text-primary">{title}</h2>
          {subtitle && <p className="truncate text-xs text-muted">{subtitle}</p>}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button type="button" onClick={() => void copyAll()} disabled={copyState === 'copying'} className="btn-secondary min-h-11 text-xs" aria-label="Copy all output">
            {copyState === 'copied' ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
            {copyState === 'copied' ? 'Copied' : copyState === 'copying' ? 'Copying…' : copyState === 'failed' ? 'Copy failed' : 'Copy all'}
          </button>
          <button type="button" onClick={onBack} className="btn-secondary min-h-11 min-w-11 p-2" aria-label="Close live view"><ArrowLeft size={18} aria-hidden="true" /></button>
        </div>
      </header>
      {(runs.length > 1 || (alwaysListRuns && runs.length > 0)) && onSelectRun && (
        <nav aria-label="Running factory agents" className="flex gap-1 overflow-x-auto">
          {runs.map((run) => {
            const active = run.runId === runId;
            return (
              <button key={run.runId} type="button" onClick={() => onSelectRun(run)} aria-current={active ? 'true' : undefined}
                className={`flex shrink-0 items-center gap-1.5 rounded-md border px-2 py-1 text-xs ${active ? 'border-accent/50 bg-accent/15 text-primary' : 'border-subtle text-secondary hover:bg-white/5'}`}>
                <span className="h-1.5 w-1.5 rounded-full bg-good" aria-hidden="true" />
                {run.title}
              </button>
            );
          })}
        </nav>
      )}
      <div className="flex items-center justify-between gap-2 text-xs text-muted">
        <span role="status">
          {state === 'loading' ? 'Connecting to the job…'
            : state === 'gone' ? (events.length > 0 ? 'The job has ended. This is its output up to the end.' : 'No output yet. A job writes its first lines a few seconds after it starts; this view keeps checking.')
            : state === 'error' ? `Cannot read the job's output: ${error}`
            : <><span className="mr-1.5 inline-block h-2 w-2 rounded-full bg-good motion-safe:animate-pulse" aria-hidden="true" />Following · {events.length} steps</>}
        </span>
        <label className="flex items-center gap-1.5">
          <input type="checkbox" checked={follow} onChange={(event) => setFollow(event.target.checked)} />
          Follow newest
        </label>
      </div>
      </div>
      {truncated && <p className="text-[11px] text-muted">Earlier output is not shown; this view starts near the end of a long log.</p>}
      <ol className="space-y-2" aria-label="Agent output">
        {events.map((event, index) => <EventRow key={index} event={event} />)}
      </ol>
      <div ref={end} />
    </section>
  );
}
