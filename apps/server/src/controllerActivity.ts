import type { ControllerActivity, ControllerEvent } from '@git-agent-harness/contracts';

/** Reconstruct controller-launched runs from correlated events. */
export function deriveControllerActivity(events: ControllerEvent[]): ControllerActivity[] {
  const runs = new Map<string, ControllerActivity>();

  for (const event of events) {
    if (!event.run_id) continue;

    if (event.event_type === 'dispatch_started') {
      runs.set(event.run_id, {
        run_id: event.run_id,
        profile: event.profile,
        work_id: event.work_id,
        started_at: event.timestamp,
        finished_at: null,
        action: event.details,
        status: 'running',
        outcome: null
      });
    } else if (event.event_type === 'dispatch_finished' || event.event_type === 'duplicate_guard_triggered') {
      const run = runs.get(event.run_id);
      if (!run) continue;
      run.finished_at = event.timestamp;
      run.status = event.details.endsWith(': success') ? 'finished' : 'failed';
      run.outcome = event.details;
    }
  }

  return [...runs.values()]
    .sort((a, b) => b.started_at.localeCompare(a.started_at))
    .slice(0, 100);
}
