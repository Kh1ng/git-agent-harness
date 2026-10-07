import { RunningWorkersRoster } from '../components/RunningWorkersRoster.js';
import { useGahStore } from '../store/gahStore.js';
import { AgentLiveView } from '../components/AgentLiveView.js';

/**
 * The Running agents sidebar: every factory agent at work right now, and a
 * read-only live view of the one selected. It opens on the list; picking an
 * agent shows its output, and Back returns to the list.
 */
export function RunningAgentsPanel({ selectedRunId, onSelect, onOpenWork }: {
  onOpenWork?: (workId: string) => void;
  selectedRunId: string | null;
  onSelect: (runId: string | null) => void;
}) {
  const status = useGahStore((state) => state.status.data);
  const workers = status?.running_workers ?? [];
  const runs = workers.map(worker => ({ runId: worker.run_id, title: `${worker.backend} ${worker.model ?? 'Unknown model'} on ${worker.work_id ?? 'Unknown work'}`, subtitle: worker.mode }));

  if (selectedRunId) {
    // A selected job stays on screen after it ends, so its last output can be read and copied.
    const selected = runs.find((run) => run.runId === selectedRunId);
    return <AgentLiveView runId={selectedRunId} title={selected?.title ?? 'Finished job'} subtitle={selected ? selected.subtitle : 'No longer running'}
      onBack={() => onSelect(null)} runs={runs} onSelectRun={(run) => onSelect(run.runId)} alwaysListRuns />;
  }
  return <RunningWorkersRoster workers={workers} onOpenWork={onOpenWork} onWatch={worker => onSelect(worker.run_id)} />;
}
