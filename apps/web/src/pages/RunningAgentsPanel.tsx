import { useEffect, useMemo, useState } from 'react';
import { Bot, ChevronRight } from 'lucide-react';
import type { ControllerActivity, DeviceAgent, Session } from '@git-agent-harness/contracts';
import { useGahStore } from '../store/gahStore.js';
import { AgentLiveView, type WatchableRun } from '../components/AgentLiveView.js';
import { buildLiveRows, formatDuration, liveAccounts, liveRowTitle } from '../components/LiveAgentsCard.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';

/**
 * The Running agents sidebar: every factory agent at work right now, and a
 * read-only live view of the one selected. It opens on the list; picking an
 * agent shows its output, and Back returns to the list.
 */
export function RunningAgentsPanel({ profile, sessions, controllerRuns, factoryAgents, selectedRunId, onSelect }: {
  profile: string | null;
  sessions: Session[];
  controllerRuns: ControllerActivity[];
  factoryAgents: DeviceAgent[] | undefined;
  selectedRunId: string | null;
  onSelect: (runId: string | null) => void;
}) {
  const status = useGahStore((state) => state.status.data);
  const quota = useGahStore((state) => state.quota.data);
  const fetchStatus = useGahStore((state) => state.fetchStatus);
  useEffect(() => { fetchStatus(profile ?? undefined); }, [profile, fetchStatus, controllerRuns.length]);

  const runs = useMemo<(WatchableRun & { since: string | null; job: string | null })[]>(() => {
    const accounts = liveAccounts([], quota?.candidates ?? [], Date.now());
    return buildLiveRows({ accounts, sessions, controllerRuns, claims: status?.active_claims ?? [], ledgers: {}, factoryAgents })
      .filter((row) => row.runId)
      .map((row) => ({ runId: row.runId!, title: liveRowTitle(row), subtitle: row.mode, since: row.since, job: row.job }));
  }, [quota, sessions, controllerRuns, status, factoryAgents]);

  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (selectedRunId) {
    // A selected job stays on screen after it ends, so its last output can be read and copied.
    const selected = runs.find((run) => run.runId === selectedRunId);
    return <AgentLiveView runId={selectedRunId} title={selected?.title ?? 'Finished job'} subtitle={selected ? selected.subtitle : 'No longer running'}
      onBack={() => onSelect(null)} runs={runs} onSelectRun={(run) => onSelect(run.runId)} alwaysListRuns />;
  }
  return (
    <div className="space-y-4">
      <PageHeader title="Running agents" description="Factory agents at work right now. Pick one to watch its output." />
      {runs.length === 0 ? (
        <EmptyState icon={Bot} title="No factory agent is running" description="When the loop dispatches a job, it appears here with its live output." />
      ) : (
        <ul className="card divide-y divide-subtle" aria-label="Running factory agents">
          {runs.map((run) => (
            <li key={run.runId}>
              <button type="button" onClick={() => onSelect(run.runId)} className="flex w-full items-start gap-3 px-3 py-2.5 text-left hover:bg-white/5">
                <span className="mt-1.5 h-2.5 w-2.5 shrink-0 rounded-full bg-good" role="img" aria-label="running" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-semibold text-primary">{run.title}</span>
                  <span className="block truncate text-xs tabular-nums text-muted">
                    {[run.subtitle, run.since ? `running for ${formatDuration(now - Date.parse(run.since))}` : null].filter(Boolean).join(' · ')}
                  </span>
                </span>
                <ChevronRight size={16} className="mt-1 shrink-0 text-muted" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
