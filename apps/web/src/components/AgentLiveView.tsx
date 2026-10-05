import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, FileEdit, Terminal } from 'lucide-react';
import type { FactoryRunEvent } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

const POLL_MS = 2000;
/** Keep the page light on a long job: older steps fall off the top. */
const MAX_EVENTS = 400;

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
export function AgentLiveView({ runId, title, subtitle, onBack }: { runId: string; title: string; subtitle?: string | null; onBack: () => void }) {
  const [events, setEvents] = useState<FactoryRunEvent[]>([]);
  const [state, setState] = useState<'loading' | 'live' | 'gone' | 'error'>('loading');
  const [error, setError] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [follow, setFollow] = useState(true);
  const next = useRef(0);
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let current = true;
    next.current = 0;
    setEvents([]); setState('loading'); setTruncated(false);
    const poll = async () => {
      try {
        const output = await gahApi.getFactoryRunOutput(runId, next.current);
        if (!current) return;
        if (!output.found) { setState((previous) => (previous === 'loading' ? 'gone' : previous === 'live' ? 'gone' : previous)); return; }
        next.current = output.next;
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
    void poll();
    const timer = window.setInterval(poll, POLL_MS);
    return () => { current = false; window.clearInterval(timer); };
  }, [runId]);

  useEffect(() => { if (follow) end.current?.scrollIntoView({ block: 'end' }); }, [events, follow]);

  return (
    <section aria-label={`Live view of ${title}`} className="flex min-w-0 flex-col gap-3">
      <header className="flex items-start justify-between gap-3 border-b border-subtle pb-3">
        <div className="min-w-0">
          <p className="text-[11px] font-semibold uppercase tracking-wide text-muted">Live view · read only</p>
          <h2 className="mt-0.5 break-words text-base font-semibold text-primary">{title}</h2>
          {subtitle && <p className="truncate text-xs text-muted">{subtitle}</p>}
        </div>
        <button type="button" onClick={onBack} className="btn-secondary min-h-11 min-w-11 p-2" aria-label="Close live view"><ArrowLeft size={18} aria-hidden="true" /></button>
      </header>
      <div className="flex items-center justify-between gap-2 text-xs text-muted">
        <span role="status">
          {state === 'loading' ? 'Connecting to the job…'
            : state === 'gone' ? (events.length > 0 ? 'The job has ended. This is its output up to the end.' : 'No running job has this output open on this node.')
            : state === 'error' ? `Cannot read the job's output: ${error}`
            : <><span className="mr-1.5 inline-block h-2 w-2 rounded-full bg-good motion-safe:animate-pulse" aria-hidden="true" />Following · {events.length} steps</>}
        </span>
        <label className="flex items-center gap-1.5">
          <input type="checkbox" checked={follow} onChange={(event) => setFollow(event.target.checked)} />
          Follow newest
        </label>
      </div>
      {truncated && <p className="text-[11px] text-muted">Earlier output is not shown; this view starts near the end of a long log.</p>}
      <ol className="space-y-2" aria-label="Agent output">
        {events.map((event, index) => <EventRow key={index} event={event} />)}
      </ol>
      <div ref={end} />
    </section>
  );
}
