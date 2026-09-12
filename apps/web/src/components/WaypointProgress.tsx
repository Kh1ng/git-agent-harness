import { Check, Circle, CircleDot } from 'lucide-react';
import type { LedgerEntry, MergeRequest, WorkWaypointEvidence as LedgerWaypointEvidence } from '@git-agent-harness/contracts';

export const WAYPOINTS = [
  { key: 'triaged', label: 'Triaged' },
  { key: 'in_progress', label: 'In progress' },
  { key: 'implemented', label: 'Implemented' },
  { key: 'validated', label: 'Validated' },
  { key: 'pull_request', label: 'PR' },
  { key: 'merged', label: 'Merged' }
] as const;

export type WaypointKey = (typeof WAYPOINTS)[number]['key'];

export interface WaypointEvidence {
  priorAttemptCount?: number;
  hasActiveClaim?: boolean;
  hasActiveMergeRequest?: boolean;
  humanRequired?: boolean;
  sessionStatus?: 'starting' | 'running' | 'stopped' | 'error';
  sessionStartedAt?: string;
  ledgerEvidence?: LedgerWaypointEvidence;
  entries?: LedgerEntry[];
  mergeRequest?: MergeRequest;
}

export interface WorkWaypoint {
  key: WaypointKey;
  label: string;
  reached: boolean;
  current: boolean;
  timestamp: string | null;
  evidence: string;
}

function firstEntry(entries: LedgerEntry[], predicate: (entry: LedgerEntry) => boolean): LedgerEntry | undefined {
  return entries.find(predicate);
}

/** Derive lifecycle progress from the status, ledger, session, and provider
 * records GAH already owns. This intentionally stores no parallel state. */
export function deriveWorkWaypoints(evidence: WaypointEvidence): WorkWaypoint[] {
  const allEntries = evidence.entries ?? [];
  const lastReset = allEntries.reduce((latest, entry, index) => entry.mode === 'clear_attempts' ? index : latest, -1);
  const entries = allEntries.slice(lastReset + 1);
  const ledgerEvidence = evidence.ledgerEvidence;
  const mergeRequest = evidence.mergeRequest;
  const firstDispatch = firstEntry(entries, (entry) =>
    entry.dispatch_reason != null
    || (entry.attempts_started ?? 0) > 0
    || entry.backend_exit_code != null
    || entry.commit_attempted
    || entry.validation_result != null
  );
  const firstCommit = firstEntry(entries, (entry) => entry.commit_created);
  const firstValidation = firstEntry(entries, (entry) => entry.validation_result === 'passed');
  const firstPullRequest = firstEntry(entries, (entry) => entry.mr_created);
  const merged = mergeRequest?.merged === true;
  const hasPullRequest = evidence.hasActiveMergeRequest === true
    || mergeRequest !== undefined
    || firstPullRequest !== undefined
    || ledgerEvidence?.first_pull_request_at != null;
  const isRunning = evidence.sessionStatus === 'starting' || evidence.sessionStatus === 'running';

  const rows: Omit<WorkWaypoint, 'current'>[] = [
    {
      key: 'triaged',
      label: 'Triaged',
      reached: true,
      timestamp: null,
      evidence: 'Present in the project work queue'
    },
    {
      key: 'in_progress',
      label: 'In progress',
      reached: isRunning || evidence.hasActiveClaim === true || (evidence.priorAttemptCount ?? 0) > 0 || entries.length > 0 || ledgerEvidence?.first_dispatch_at != null || hasPullRequest,
      timestamp: firstDispatch?.timestamp ?? ledgerEvidence?.first_dispatch_at ?? evidence.sessionStartedAt ?? null,
      evidence: firstDispatch ? 'First durable dispatch attempt' : isRunning || evidence.hasActiveClaim ? 'Active session or claim' : 'Dispatch history exists'
    },
    {
      key: 'implemented',
      label: 'Implemented',
      reached: firstCommit !== undefined || ledgerEvidence?.first_commit_at != null || hasPullRequest,
      timestamp: firstCommit?.timestamp ?? ledgerEvidence?.first_commit_at ?? null,
      evidence: firstCommit || ledgerEvidence?.first_commit_at ? 'Commit recorded in the ledger' : 'Provider pull request exists'
    },
    {
      key: 'validated',
      label: 'Validated',
      reached: firstValidation !== undefined || ledgerEvidence?.first_validation_at != null || mergeRequest?.ci_passed === true || merged,
      timestamp: firstValidation?.timestamp ?? ledgerEvidence?.first_validation_at ?? null,
      evidence: firstValidation || ledgerEvidence?.first_validation_at ? 'Validation passed in the ledger' : mergeRequest?.ci_passed ? 'Provider CI passed' : 'Merge proves the validation gate completed'
    },
    {
      key: 'pull_request',
      label: 'PR',
      reached: hasPullRequest,
      timestamp: firstPullRequest?.timestamp ?? ledgerEvidence?.first_pull_request_at ?? null,
      evidence: mergeRequest?.id ? `Provider PR/MR ${mergeRequest.id}` : 'Pull request recorded'
    },
    {
      key: 'merged',
      label: 'Merged',
      reached: merged,
      timestamp: mergeRequest?.merged_at ?? null,
      evidence: mergeRequest?.merge_commit_sha ? `Merge commit ${mergeRequest.merge_commit_sha.slice(0, 8)}` : 'Provider reports merged'
    }
  ];

  const currentIndex = rows.reduce((latest, row, index) => row.reached ? index : latest, 0);
  return rows.map((row, index) => ({ ...row, current: index === currentIndex }));
}

export function currentWorkWaypoint(evidence: WaypointEvidence): WorkWaypoint {
  const waypoints = deriveWorkWaypoints(evidence);
  return waypoints.find((waypoint) => waypoint.current) ?? waypoints[0];
}

function markerClass(waypoint: WorkWaypoint, blocked: boolean): string {
  if (waypoint.current && blocked) return 'border-warning bg-warning/15 text-warning';
  if (waypoint.current) return 'border-accent bg-accent/15 text-accent';
  if (waypoint.reached) return 'border-good bg-good/15 text-good';
  return 'border-subtle bg-card text-muted';
}

export function WaypointStrip({ evidence, label }: { evidence: WaypointEvidence; label: string }) {
  const waypoints = deriveWorkWaypoints(evidence);
  const current = waypoints.find((waypoint) => waypoint.current) ?? waypoints[0];
  return (
    <div>
      <ol className="grid min-w-[250px] grid-cols-6 gap-1 sm:min-w-[430px]" aria-label={`${label} waypoints`}>
        {waypoints.map((waypoint) => (
          <li
            key={waypoint.key}
            aria-current={waypoint.current ? 'step' : undefined}
            aria-label={`${waypoint.label}: ${waypoint.current ? 'current' : waypoint.reached ? 'reached' : 'not reached'}`}
          >
            <span
              className={`block h-1.5 rounded-sm ${waypoint.current ? 'bg-accent' : waypoint.reached ? 'bg-good' : 'bg-subtle'}`}
              aria-hidden="true"
            />
            <span className={`mt-1 hidden text-[11px] leading-tight sm:block ${waypoint.current ? 'font-semibold text-primary' : waypoint.reached ? 'text-secondary' : 'text-muted'}`}>
              {waypoint.label}
            </span>
          </li>
        ))}
      </ol>
      <span className="mt-1 block text-[11px] font-medium text-primary sm:hidden">Current: {current.label}</span>
    </div>
  );
}

export function WaypointHistory({ evidence, label }: { evidence: WaypointEvidence; label: string }) {
  const waypoints = deriveWorkWaypoints(evidence);
  return (
    <section className="card-padded" aria-labelledby="waypoint-history-title">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <h3 id="waypoint-history-title" className="text-sm font-semibold text-primary">Work waypoints</h3>
          <p className="mt-1 text-xs text-muted">Derived from dispatch, validation, and provider records.</p>
        </div>
        {evidence.humanRequired && <span className="badge badge-warning">Action required</span>}
      </div>
      <ol className="mt-5 grid grid-cols-1 gap-0 sm:grid-cols-6" aria-label={`${label} waypoint history`}>
        {waypoints.map((waypoint, index) => {
          const Icon = waypoint.current ? CircleDot : waypoint.reached ? Check : Circle;
          return (
            <li
              key={waypoint.key}
              aria-current={waypoint.current ? 'step' : undefined}
              className="relative flex min-h-20 gap-3 pb-4 sm:block sm:min-h-0 sm:pb-0 sm:pr-3"
            >
              {index < waypoints.length - 1 && (
                <span className={`absolute left-[11px] top-6 h-[calc(100%-1rem)] w-px sm:left-6 sm:right-0 sm:top-[11px] sm:h-px sm:w-auto ${waypoint.reached && waypoints[index + 1].reached ? 'bg-good' : 'bg-subtle'}`} aria-hidden="true" />
              )}
              <span className={`relative z-10 flex h-6 w-6 shrink-0 items-center justify-center rounded-full border ${markerClass(waypoint, evidence.humanRequired === true)}`}>
                <Icon size={12} aria-hidden="true" />
              </span>
              <div className="min-w-0 sm:mt-2">
                <span className={`block text-xs font-medium ${waypoint.current ? 'text-primary' : 'text-secondary'}`}>{waypoint.label}</span>
                <span className="mt-1 block text-[11px] leading-snug text-muted">{waypoint.reached ? waypoint.evidence : 'Not reached'}</span>
                {waypoint.timestamp && <time className="mt-1 block break-words font-mono text-[10px] text-muted" dateTime={waypoint.timestamp}>{waypoint.timestamp}</time>}
              </div>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
