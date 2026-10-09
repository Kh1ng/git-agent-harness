import { useEffect, useRef, useState } from 'react';
import { formatDuration } from '../lib/format.js';
import type { KanbanAgent, WorkingJob } from '../lib/kanbanBoard.js';

const STATE_DOT: Record<KanbanAgent['state'], string> = { working: 'bg-good', idle: 'bg-muted', unavailable: 'bg-critical' };
const STATE_LABEL: Record<KanbanAgent['state'], string> = { working: 'Working', idle: 'Idle', unavailable: 'Unavailable' };

/**
 * "Working on #1367" at the right of the navbar: the job the factory is on
 * now. Hovering or clicking it lists every running job with its agent, and
 * each agent with what it is doing or why it is idle.
 */
export function WorkingNowMenu({ jobs, agents, onOpenBoard }: { jobs: WorkingJob[]; agents: KanbanAgent[]; onOpenBoard: () => void }) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const menu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !menu.current?.contains(event.target as Node)) { setOpen(false); setPinned(false); }
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => { window.clearInterval(timer); document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close); };
  }, [open]);

  if (jobs.length === 0 && agents.length === 0) return null;
  const first = jobs[0];
  const summary = !first ? 'Idle' : `Working on ${first.workId ?? first.label}${jobs.length > 1 ? ` +${jobs.length - 1}` : ''}`;
  return (
    <div className="relative hidden items-center sm:flex" ref={menu} onMouseLeave={() => { if (!pinned) setOpen(false); }}>
      <button type="button" aria-expanded={open} aria-haspopup="true"
        aria-label={first ? `${summary}: ${jobs.length} ${jobs.length === 1 ? 'job' : 'jobs'} running` : 'No job is running'}
        onMouseEnter={() => { if (!pinned) { setOpen(true); setNow(Date.now()); } }}
        onClick={() => { setPinned(!open || !pinned); setOpen(true); setNow(Date.now()); }}
        className={`flex h-9 items-center gap-1.5 whitespace-nowrap rounded-md px-2 text-xs hover:bg-overlay/5 ${open ? 'bg-overlay/5' : ''} ${first ? 'text-primary' : 'text-muted'}`}>
        <span className={`h-2 w-2 shrink-0 rounded-full ${first ? 'animate-pulse bg-good' : 'bg-muted'}`} aria-hidden="true" />
        {summary}
      </button>
      {open && (
        <div className="absolute right-0 top-full z-40 mt-1 w-80 max-w-[calc(100vw-1.5rem)] rounded-lg border border-subtle bg-raised shadow-xl" role="dialog" aria-label="What the factory is doing now">
          <div className="px-3 py-2">
            <h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted">Running now</h2>
            {jobs.length === 0 && <p className="mt-1 text-xs text-secondary">Nothing is running.</p>}
            <ul className="mt-1 space-y-1.5">
              {jobs.map((job) => (
                <li key={job.runId} className="text-xs">
                  <span className="font-medium text-primary">{job.label}</span>
                  <span className="block text-[11px] text-muted">{job.agent ?? 'Agent not visible yet'} · {formatDuration(Math.max(0, (now - Date.parse(job.since)) / 1000))}</span>
                </li>
              ))}
            </ul>
          </div>
          {agents.length > 0 && (
            <div className="border-t border-subtle px-3 py-2">
              <h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted">Agents</h2>
              <ul className="mt-1 space-y-1.5">
                {agents.map((agent) => (
                  <li key={agent.id} className="flex items-start gap-2 text-xs">
                    <span className={`mt-1 h-2 w-2 shrink-0 rounded-full ${STATE_DOT[agent.state]}`} role="img" aria-label={STATE_LABEL[agent.state]} />
                    <span className="min-w-0">
                      <span className="block break-words font-medium text-primary">{agent.name}</span>
                      <span className="block text-[11px] text-muted">{agent.reason}</span>
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <button type="button" className="w-full border-t border-subtle px-3 py-2 text-left text-xs font-medium text-accent hover:underline" onClick={() => { setOpen(false); setPinned(false); onOpenBoard(); }}>
            Open the Kanban board
          </button>
        </div>
      )}
    </div>
  );
}
