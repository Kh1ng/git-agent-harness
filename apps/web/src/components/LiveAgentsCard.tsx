import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Radio } from 'lucide-react';
import type { ActiveClaim, BackendInstanceSummary, ControllerActivity, LedgerEntry, QuotaCandidateStatus, Session } from '@git-agent-harness/contracts';
import { backendInstancesApi, gahApi } from '../api/client.js';
import { formatLocalTime } from '../lib/format.js';

/** What an agent account is doing right now, in the order the row is sorted. */
export type LiveState = 'working' | 'gates' | 'paused' | 'down' | 'halted' | 'idle';

export interface LiveAgentRow {
  key: string;
  name: string;
  /** Runner and model, or the account label's backend. */
  detail: string | null;
  state: LiveState;
  /** Work id or controller action for a busy row. */
  job: string | null;
  mode: string | null;
  /** ISO time the job started; drives the elapsed timer. */
  since: string | null;
  /** The active claim's age in seconds at the last status refresh. */
  claimAgeSeconds: number | null;
  /** Quota resume time for a paused row. */
  resumes: string | null;
  reason: string | null;
}

const RANK: Record<LiveState, number> = { working: 0, gates: 0, paused: 1, down: 1, halted: 1, idle: 2 };
const DOT: Record<LiveState, { className: string; pulse: boolean; label: string }> = {
  working: { className: 'bg-good', pulse: true, label: 'working' },
  gates: { className: 'bg-accent', pulse: true, label: 'running gates' },
  paused: { className: 'bg-warning', pulse: false, label: 'paused' },
  idle: { className: 'bg-muted/40', pulse: false, label: 'idle' },
  halted: { className: 'bg-critical', pulse: false, label: 'halted' },
  down: { className: 'bg-critical', pulse: false, label: 'down' }
};

/** `1h 05m`, `4m 20s`, `12s`; never negative. */
export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h) return `${h}h ${String(m).padStart(2, '0')}m`;
  if (m) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${s}s`;
}

const GATE_MODES = new Set(['review', 'validate', 'validation', 'merge']);

function sessionMatches(session: Session, instance: BackendInstanceSummary): boolean {
  if (session.instanceId === instance.backend_instance) return true;
  const backend = session.backend ?? session.providerKind;
  return backend === instance.logical_backend || backend === instance.backend_instance;
}

/**
 * One row per configured agent account (backend instance), from what the
 * dashboard already holds: the live session list, running controller runs,
 * the status snapshot's active claims, and the quota snapshot's availability.
 * A busy session on an unconfigured backend gets a row of its own.
 */
export function buildLiveRows(input: {
  instances: BackendInstanceSummary[];
  sessions: Session[];
  controllerRuns: ControllerActivity[];
  claims: ActiveClaim[];
  candidates: QuotaCandidateStatus[];
  now: number;
}): LiveAgentRow[] {
  const busy = input.sessions.filter((session) => ['starting', 'running', 'stopping'].includes(session.status));
  const claimed = new Set<string>();
  const rows: LiveAgentRow[] = [];
  const claimFor = (workId: string | null | undefined) => (workId ? input.claims.find((claim) => claim.work_id === workId) ?? null : null);

  for (const instance of input.instances) {
    const session = busy.find((candidate) => !claimed.has(candidate.id) && sessionMatches(candidate, instance)) ?? null;
    if (session) claimed.add(session.id);
    const candidate = input.candidates.find((item) => item.backend_instance === instance.backend_instance)
      ?? input.candidates.find((item) => !item.backend_instance && item.backend === instance.logical_backend) ?? null;
    const resumes = candidate && !candidate.eligible_now && candidate.unavailable_until && Date.parse(candidate.unavailable_until) > input.now
      ? candidate.unavailable_until : null;
    const unhealthy = instance.healthy === false || instance.auth_ready === false || instance.executable_resolved === false;
    const state: LiveState = session ? (session.mode && GATE_MODES.has(session.mode) ? 'gates' : 'working')
      : !instance.enabled ? 'halted'
      : unhealthy ? 'down'
      : resumes ? 'paused'
      : 'idle';
    const claim = claimFor(session?.target);
    rows.push({
      key: instance.backend_instance,
      name: instance.account_label ?? instance.backend_instance,
      detail: [instance.logical_backend, session?.model ?? instance.supported_models[0]].filter(Boolean).join(' · ') || null,
      state,
      job: session?.target ?? null,
      mode: session?.mode ?? null,
      since: session?.startedAt ?? null,
      claimAgeSeconds: claim?.age_seconds ?? null,
      resumes,
      reason: state === 'down' ? instance.resolution_error ?? 'not ready' : state === 'paused' ? candidate?.reason ?? null : null
    });
  }

  for (const session of busy) {
    if (claimed.has(session.id)) continue;
    const claim = claimFor(session.target);
    rows.push({
      key: `session:${session.id}`,
      name: session.instanceId || session.backend || session.providerKind,
      detail: [session.backend ?? session.providerKind, session.model].filter(Boolean).join(' · ') || null,
      state: session.mode && GATE_MODES.has(session.mode) ? 'gates' : 'working',
      job: session.target ?? null,
      mode: session.mode ?? null,
      since: session.startedAt ?? null,
      claimAgeSeconds: claim?.age_seconds ?? null,
      resumes: null,
      reason: null
    });
  }

  for (const run of input.controllerRuns) {
    if (run.status !== 'running') continue;
    const claim = claimFor(run.work_id);
    rows.push({
      key: `run:${run.run_id}`,
      name: 'controller',
      detail: run.action.replace(/^dispatch:\s*/i, ''),
      state: 'working',
      job: run.work_id,
      mode: null,
      since: run.started_at,
      claimAgeSeconds: claim?.age_seconds ?? null,
      resumes: null,
      reason: null
    });
  }

  return rows
    .map((row, index) => ({ row, index }))
    .sort((a, b) => RANK[a.row.state] - RANK[b.row.state] || a.index - b.index)
    .map(({ row }) => row);
}

function LiveRow({ row, now, ledger }: { row: LiveAgentRow; now: number; ledger: LedgerEntry | null | undefined }) {
  const dot = DOT[row.state];
  const job = row.job ? <span className="font-mono text-primary">{row.job}</span> : 'the next job';
  let line: ReactNode;
  if (row.state === 'working') {
    line = <>{row.mode ? `${row.mode} on ` : 'working on '}{job}{row.since && <> for {formatDuration(now - Date.parse(row.since))}</>}</>;
  } else if (row.state === 'gates') {
    line = <>{row.mode ?? 'gates'} on {job}{row.since && <> for {formatDuration(now - Date.parse(row.since))}</>}</>;
  } else if (row.state === 'paused') {
    line = <>paused on quota, resumes in {formatDuration(Date.parse(row.resumes!) - now)} ({formatLocalTime(row.resumes) ?? row.resumes})</>;
  } else if (row.state === 'halted') {
    line = 'disabled, routing skips it';
  } else if (row.state === 'down') {
    line = <>not ready{row.reason ? `: ${row.reason}` : ''}</>;
  } else {
    line = 'idle, waiting for the router';
  }
  const busy = row.state === 'working' || row.state === 'gates';
  const files = ledger?.files_changed;
  const edits = busy ? [
    files != null ? `${files} file${files === 1 ? '' : 's'} changed` : null,
    row.claimAgeSeconds != null ? `claimed ${formatDuration(row.claimAgeSeconds * 1000)} ago` : null,
    ledger?.timestamp ? `last attempt ${formatDuration(now - Date.parse(ledger.timestamp))} ago` : null
  ].filter(Boolean).join(' · ') : '';
  return (
    <li className="grid grid-cols-[12px_minmax(5rem,10rem)_minmax(0,1fr)] items-start gap-3 py-2" data-live-state={row.state}>
      <span className={`mt-1.5 h-2.5 w-2.5 rounded-full ${dot.className} ${dot.pulse ? 'motion-safe:animate-pulse' : ''}`} role="img" aria-label={dot.label} />
      <div className="min-w-0">
        <p className="truncate text-sm font-semibold text-primary" title={row.name}>{row.name}</p>
        {row.detail && <p className="truncate text-[11px] text-muted" title={row.detail}>{row.detail}</p>}
      </div>
      <div className="min-w-0">
        <p className="text-sm tabular-nums text-secondary">{line}</p>
        {edits && <p className="truncate text-xs tabular-nums text-muted">{edits}</p>}
      </div>
    </li>
  );
}

/**
 * Live: what each agent account is doing now. Rows come from
 * `buildLiveRows`; the elapsed timers tick every second while any row is
 * busy, and a busy row's latest ledger entry supplies its file count.
 */
export function LiveAgentsCard({ profile, sessions, controllerRuns, claims, candidates }: {
  profile: string | null;
  sessions: Session[];
  controllerRuns: ControllerActivity[];
  claims: ActiveClaim[];
  candidates: QuotaCandidateStatus[];
}) {
  const [instances, setInstances] = useState<BackendInstanceSummary[] | null>(null);
  const [instancesError, setInstancesError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [ledgers, setLedgers] = useState<Record<string, LedgerEntry | null>>({});

  useEffect(() => {
    if (!profile) return;
    let cancelled = false;
    backendInstancesApi.list(profile)
      .then(({ backend_instances }) => { if (!cancelled) { setInstances(backend_instances); setInstancesError(null); } })
      .catch((err) => { if (!cancelled) { setInstances([]); setInstancesError(err instanceof Error ? err.message : String(err)); } });
    return () => { cancelled = true; };
  }, [profile, sessions.length]);

  const rows = useMemo(() => buildLiveRows({ instances: instances ?? [], sessions, controllerRuns, claims, candidates, now }),
    // `now` only matters for the paused cut-off; the timer below re-renders rows anyway.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [instances, sessions, controllerRuns, claims, candidates]);
  const busyJobs = useMemo(() => [...new Set(rows.filter((row) => (row.state === 'working' || row.state === 'gates') && row.job).map((row) => row.job!))].slice(0, 8), [rows]);

  useEffect(() => {
    if (busyJobs.length === 0) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [busyJobs.length]);

  // Each busy job's latest ledger entry carries the files it changed so far.
  useEffect(() => {
    let cancelled = false;
    for (const job of busyJobs) {
      if (job in ledgers) continue;
      gahApi.getWorkTimeline(job)
        .then((entries) => { if (!cancelled) setLedgers((current) => ({ ...current, [job]: entries.at(-1) ?? null })); })
        .catch(() => { if (!cancelled) setLedgers((current) => ({ ...current, [job]: null })); });
    }
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busyJobs]);

  const busy = rows.filter((row) => row.state === 'working' || row.state === 'gates').length;
  return (
    <section className="card-padded" aria-labelledby="live-agents-title">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h3 id="live-agents-title" className="flex items-center gap-2 text-sm font-semibold text-primary">
          <Radio size={15} className="text-accent" aria-hidden="true" />
          Live: what each agent is doing now
        </h3>
        <span className="text-xs tabular-nums text-muted">{busy > 0 ? `${busy} busy of ${rows.length}` : rows.length > 0 ? `${rows.length} idle` : ''}</span>
      </div>
      {instancesError && <p role="alert" className="mb-2 text-xs text-critical">Cannot load agent accounts: {instancesError}</p>}
      {instances === null && !instancesError ? (
        <p className="text-sm text-muted">Loading agent accounts…</p>
      ) : rows.length === 0 ? (
        <p className="text-sm text-muted">No agent accounts configured for this profile and nothing running.</p>
      ) : (
        <ul className="divide-y divide-subtle" aria-label="Agents">
          {rows.map((row) => <LiveRow key={row.key} row={row} now={now} ledger={row.job ? ledgers[row.job] : null} />)}
        </ul>
      )}
    </section>
  );
}
