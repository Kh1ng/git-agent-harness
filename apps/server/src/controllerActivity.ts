import type { ControllerActivity, ControllerEvent, LoopDecision } from '@git-agent-harness/contracts';

/** Whether terminal controller details report a successful dispatch. */
export function controllerDispatchSucceeded(details: string): boolean {
  return /:\s*success\s*$/i.test(details);
}

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
      run.status = controllerDispatchSucceeded(event.details) ? 'finished' : 'failed';
      run.outcome = event.details;
      if (event.skipped?.length) run.skipped = event.skipped;
    }
  }

  return [...runs.values()]
    .sort((a, b) => b.started_at.localeCompare(a.started_at))
    .slice(0, 100);
}

/**
 * The loop's latest decision, from `action_decided` events (details
 * `kind: reason`) and `action_overridden` events (`from -> kind: reason`),
 * which replace the decision they follow.
 */
export function deriveLastDecision(events: ControllerEvent[]): LoopDecision | null {
  let latest: LoopDecision | null = null;
  for (const event of events) {
    if (event.event_type !== 'action_decided' && event.event_type !== 'action_overridden') continue;
    if (latest && event.timestamp < latest.timestamp) continue;
    const details = event.details.replace(/ review_generation=\S+$/, '');
    const match = /^(?:\S+ -> )?(\w+): ([\s\S]*)$/.exec(details);
    if (!match) continue;
    latest = {
      timestamp: event.timestamp,
      kind: match[1],
      reason: match[2],
      work_id: event.work_id,
      reason_code: event.reason_code ?? null
    };
  }
  return latest;
}
