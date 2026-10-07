import type { NodeObservationSnapshot, RunningWorker, StatusSnapshot } from '@git-agent-harness/contracts';

/** Flatten source-node observations only; nested remote rosters are never relayed. */
export function runningWorkers(status: StatusSnapshot, nodes: NodeObservationSnapshot[], nodeId: string, now = Date.now()): RunningWorker[] {
  const rows = (status.running_workers ?? []).map(worker => ({ ...worker, node_id: nodeId }));
  for (const node of nodes) {
    if (node.node_id === nodeId) continue;
    rows.push(...(node.running_workers ?? [])
      .filter(worker => (worker.profile ?? node.profile) === status.profile.profile)
      .map(worker => ({ ...worker, node_id: node.node_id,
        state: worker.profile || node.state === 'healthy' ? worker.state : 'stale' as const })));
  }
  const unique = new Map<string, RunningWorker>();
  for (const row of rows) {
    if (now - Date.parse(row.last_activity_at) >= row.stale_after_seconds * 1000) row.state = 'stale';
    unique.set(`${row.node_id}:${row.run_id}:${row.attempt}`, row);
  }
  return [...unique.values()];
}
