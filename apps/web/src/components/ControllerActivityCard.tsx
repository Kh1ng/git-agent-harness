import { Activity, AlertCircle, CheckCircle2, ChevronRight, Clock3 } from 'lucide-react';
import type { ControllerActivity } from '@git-agent-harness/contracts';
import { formatAge } from '../lib/format.js';
import { StatusBadge } from './ui/StatusBadge.js';

function tone(status: ControllerActivity['status']) {
  if (status === 'running') return 'good' as const;
  if (status === 'failed') return 'critical' as const;
  return 'unknown' as const;
}

function ActivityRow({ run }: { run: ControllerActivity }) {
  const timestamp = formatAge(run.finished_at ?? run.started_at);

  return (
    <div className="flex items-start justify-between gap-3 px-4 py-3">
      <div className="flex min-w-0 flex-1 items-start gap-2">
        {run.status === 'running'
          ? <Clock3 size={14} className="mt-0.5 shrink-0" aria-hidden="true" />
          : run.status === 'failed'
            ? <AlertCircle size={14} className="mt-0.5 shrink-0" aria-hidden="true" />
            : <CheckCircle2 size={14} className="mt-0.5 shrink-0" aria-hidden="true" />}
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2 text-sm text-primary">
            {run.work_id && <span className="shrink-0 font-mono text-xs">{run.work_id}</span>}
            <span className="truncate" title={run.action}>{run.action}</span>
          </div>
          <p className="mt-1 truncate text-xs text-muted" title={`run ${run.run_id}${run.outcome ? ` · ${run.outcome}` : ''}`}>
            run {run.run_id.slice(0, 8)}{timestamp ? ` · ${timestamp}` : ''}{run.outcome ? ` · ${run.outcome}` : ''}
          </p>
        </div>
      </div>
      <StatusBadge tone={tone(run.status)} label={run.status} />
    </div>
  );
}

export function ControllerActivityCard({ activity }: { activity: ControllerActivity[] }) {
  const active = activity.filter((run) => run.status === 'running');
  const recent = activity.filter((run) => run.status !== 'running').slice(0, 5);
  const failed = recent.filter((run) => run.status === 'failed').length;
  const finished = recent.length - failed;

  return (
    <section aria-labelledby="controller-activity-heading">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 id="controller-activity-heading" className="flex items-center gap-2 text-sm font-semibold text-primary">
          <Activity size={15} className="text-accent" aria-hidden="true" />
          Controller activity
        </h3>
        <StatusBadge tone={active.length > 0 ? 'good' : 'unknown'} label={active.length > 0 ? `${active.length} running` : 'Idle'} />
      </div>

      {active.length > 0 && (
        <div className="card overflow-hidden">
          <div className="divide-y divide-subtle">
            {active.map((run) => <ActivityRow key={run.run_id} run={run} />)}
          </div>
        </div>
      )}

      {activity.length === 0 ? (
        <p className="text-sm text-muted">No controller runs in the last 24 hours.</p>
      ) : recent.length > 0 && (
        <details className={`group card overflow-hidden ${active.length > 0 ? 'mt-3' : ''}`}>
          <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-4 py-3 text-sm text-secondary hover:bg-white/5 hover:text-primary [&::-webkit-details-marker]:hidden">
            <span className="flex items-center gap-2 font-medium">
              <ChevronRight size={14} className="transition-transform group-open:rotate-90" aria-hidden="true" />
              Recent history
            </span>
            <span className="text-xs tabular-nums text-muted">
              {failed > 0 && <span className="text-critical">{failed} failed</span>}
              {failed > 0 && finished > 0 && ' · '}
              {finished > 0 && `${finished} finished`}
            </span>
          </summary>
          <div className="divide-y divide-subtle border-t border-subtle">
            {recent.map((run) => <ActivityRow key={run.run_id} run={run} />)}
          </div>
        </details>
      )}
    </section>
  );
}
