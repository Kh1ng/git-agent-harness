import { Activity, AlertCircle, CheckCircle2, Clock3 } from 'lucide-react';
import type { ControllerActivity } from '@git-agent-harness/contracts';
import { StatusBadge } from './ui/StatusBadge.js';

function tone(status: ControllerActivity['status']) {
  if (status === 'running') return 'good' as const;
  if (status === 'failed') return 'critical' as const;
  return 'unknown' as const;
}

export function ControllerActivityCard({ activity }: { activity: ControllerActivity[] }) {
  const active = activity.filter((run) => run.status === 'running');
  const recent = activity.filter((run) => run.status !== 'running').slice(0, 5);

  return (
    <section>
      <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
        <Activity size={15} className="text-accent" aria-hidden="true" />
        Controller activity ({active.length} running)
      </h3>
      {activity.length === 0 ? (
        <div className="card-padded text-sm text-secondary">No correlated controller runs in the last 24 hours.</div>
      ) : (
        <div className="card overflow-hidden">
          <div className="divide-y divide-subtle">
            {[...active, ...recent].slice(0, 8).map((run) => (
              <div key={run.run_id} className="px-4 py-3 flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm text-primary">
                    {run.status === 'running' ? <Clock3 size={14} aria-hidden="true" /> : run.status === 'failed' ? <AlertCircle size={14} aria-hidden="true" /> : <CheckCircle2 size={14} aria-hidden="true" />}
                    <span className="font-mono text-xs">{run.work_id ?? 'unassigned'}</span>
                    <span className="text-secondary">{run.action}</span>
                  </div>
                  <p className="mt-1 text-xs text-muted truncate" title={run.outcome ?? run.run_id}>
                    run {run.run_id}{run.outcome ? ` · ${run.outcome}` : ''}
                  </p>
                </div>
                <StatusBadge tone={tone(run.status)} label={run.status} />
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
