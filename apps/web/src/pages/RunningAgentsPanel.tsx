import { useEffect, useMemo } from 'react';
import { Bot } from 'lucide-react';
import type { ControllerActivity, DeviceAgent, Session } from '@git-agent-harness/contracts';
import { useGahStore } from '../store/gahStore.js';
import { AgentLiveView, type WatchableRun } from '../components/AgentLiveView.js';
import { agentDisplayName, buildLiveRows, liveAccounts } from '../components/LiveAgentsCard.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';

/**
 * The Running agents sidebar: every factory agent at work right now, and a
 * read-only live view of the one selected. With nothing chosen it shows the
 * first; with nothing running it says so.
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

  const runs = useMemo<WatchableRun[]>(() => {
    const accounts = liveAccounts([], quota?.candidates ?? [], Date.now());
    return buildLiveRows({ accounts, sessions, controllerRuns, claims: status?.active_claims ?? [], ledgers: {}, factoryAgents })
      .filter((row) => row.runId)
      .map((row) => ({ runId: row.runId!, title: `${agentDisplayName(row.name)}${row.model ? ` ${row.model}` : ''} on ${row.job ?? 'a job'}`, subtitle: row.mode }));
  }, [quota, sessions, controllerRuns, status, factoryAgents]);

  // A selected job stays on screen after it ends, so its last output can be read and copied.
  const selected = runs.find((run) => run.runId === selectedRunId) ?? (selectedRunId ? null : runs[0] ?? null);
  if (selectedRunId && !selected) {
    return <AgentLiveView runId={selectedRunId} title="Finished job" subtitle="No longer running" onBack={() => onSelect(null)} runs={runs} onSelectRun={(run) => onSelect(run.runId)} />;
  }
  if (!selected) {
    return (
      <div className="space-y-4">
        <PageHeader title="Running agents" description="Factory agents at work right now" />
        <EmptyState icon={Bot} title="No factory agent is running" description="When the loop dispatches a job, it appears here with its live output." />
      </div>
    );
  }
  return <AgentLiveView runId={selected.runId} title={selected.title} subtitle={selected.subtitle} onBack={() => onSelect(null)} runs={runs} onSelectRun={(run) => onSelect(run.runId)} alwaysListRuns />;
}
