import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Radio } from 'lucide-react';
import type { ActiveClaim, BackendInstanceSummary, ControllerActivity, DeviceAgent, LedgerEntry, QuotaCandidateStatus, Session } from '@git-agent-harness/contracts';
import { backendInstancesApi, gahApi } from '../api/client.js';
import { formatLocalTime } from '../lib/format.js';

/** What an agent account is doing right now, in the order the rows sort. */
export type LiveState = 'working' | 'gates' | 'paused' | 'down' | 'halted' | 'idle';

/** An agent account: a declared backend instance, or a routing candidate
 * from the quota snapshot when the profile declares no instances. */
export interface LiveAccount {
  id: string;
  name: string;
  backend: string;
  /** The subscription or billing provider behind the account (openai, anthropic, antigravity…). */
  provider: string | null;
  model: string | null;
  enabled: boolean;
  /** Why the account cannot take work, when it cannot. */
  notReady: string | null;
  /** Quota resume time, when routing skips it until then. */
  resumes: string | null;
  pausedReason: string | null;
}

export interface LiveAgentRow {
  key: string;
  name: string;
  /** Backend and model, or a controller run's action. */
  detail: string | null;
  provider: string | null;
  model: string | null;
  state: LiveState;
  /** Work id of a busy row. */
  job: string | null;
  mode: string | null;
  /** ISO time the job started; drives the elapsed timer. */
  since: string | null;
  /** The active claim's age in seconds at the last status refresh. */
  claimAgeSeconds: number | null;
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
const GATE_MODES = new Set(['review', 'validate', 'validation', 'merge', 'routine_review']);

/** `codex` → `Codex`: agent and model names lead with a capital. */
export function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

/** The name people know an agent by: `agy` and its instances are Antigravity. */
export function agentDisplayName(name: string): string {
  return /^agy(?:[:\-_]|$)/i.test(name) ? 'Antigravity' : capitalize(name);
}

/** `1h 05m`, `4m 20s`, `12s`; never negative. */
export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h) return `${h}h ${String(m).padStart(2, '0')}m`;
  if (m) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${s}s`;
}

/** Declared instances first; candidates fill in accounts the profile only
 * knows through routing. A candidate that names an instance refines it. */
export function liveAccounts(instances: BackendInstanceSummary[], candidates: QuotaCandidateStatus[], now: number): LiveAccount[] {
  const accounts = new Map<string, LiveAccount>();
  for (const instance of instances) {
    const unhealthy = instance.healthy === false || instance.auth_ready === false || instance.executable_resolved === false;
    accounts.set(instance.backend_instance, {
      id: instance.backend_instance,
      name: instance.account_label ?? instance.backend_instance,
      backend: instance.logical_backend,
      provider: instance.credential_provider ?? null,
      model: instance.supported_models[0] ?? null,
      enabled: instance.enabled,
      notReady: unhealthy ? instance.resolution_error ?? 'not ready' : null,
      resumes: null,
      pausedReason: null
    });
  }
  for (const candidate of candidates) {
    const id = candidate.backend_instance ?? candidate.backend;
    const resumes = !candidate.eligible_now && candidate.unavailable_until && Date.parse(candidate.unavailable_until) > now ? candidate.unavailable_until : null;
    const existing = accounts.get(id) ?? [...accounts.values()].find((account) => !candidate.backend_instance && account.backend === candidate.backend);
    if (existing) {
      existing.resumes = existing.resumes ?? resumes;
      existing.pausedReason = existing.pausedReason ?? (resumes ? candidate.reason ?? null : null);
      if (!existing.model) existing.model = candidate.model;
      if (!existing.provider) existing.provider = candidate.provider ?? null;
      continue;
    }
    accounts.set(id, {
      id,
      name: id,
      backend: candidate.backend,
      provider: candidate.provider ?? null,
      model: candidate.model,
      // A candidate is in the routing lists by definition; only a declared instance can be disabled.
      enabled: true,
      notReady: !candidate.eligible_now && !resumes ? candidate.reason ?? 'not eligible' : null,
      resumes,
      pausedReason: resumes ? candidate.reason ?? null : null
    });
  }
  return [...accounts.values()];
}

/** A job in flight: a dashboard session, a running controller run, or a
 * claim the loop holds. `backend` comes from the session, else from the
 * job's latest ledger entry once it has loaded. */
interface LiveJob {
  key: string;
  workId: string | null;
  mode: string | null;
  since: string | null;
  backend: string | null;
  instance: string | null;
  model: string | null;
  action: string | null;
}

function sessionBackend(session: Session): string | null {
  return session.backend ?? (session.providerKind as string | undefined) ?? null;
}

/**
 * One row per agent account, busy rows first, from what the dashboard
 * already holds: the live session list, running controller runs, the status
 * snapshot's active claims, and the quota snapshot's availability. A job
 * that no account explains gets a row of its own.
 */
export function buildLiveRows(input: {
  accounts: LiveAccount[];
  sessions: Session[];
  controllerRuns: ControllerActivity[];
  claims: ActiveClaim[];
  /** Latest ledger entry per busy work id, when fetched. */
  ledgers: Record<string, LedgerEntry | null | undefined>;
  /** Agent processes running in factory worktrees: the only source for a
   * running dispatch's backend, which the ledger records when it ends. */
  factoryAgents?: DeviceAgent[];
}): LiveAgentRow[] {
  const jobs: LiveJob[] = [];
  const covered = new Set<string>();
  for (const session of input.sessions) {
    if (!['starting', 'running', 'stopping'].includes(session.status)) continue;
    if (session.target) covered.add(session.target);
    jobs.push({ key: `session:${session.id}`, workId: session.target ?? null, mode: session.mode ?? null, since: session.startedAt ?? null,
      backend: sessionBackend(session), instance: session.instanceId || null, model: session.model ?? null, action: null });
  }
  for (const run of input.controllerRuns) {
    if (run.status !== 'running' || (run.work_id && covered.has(run.work_id))) continue;
    if (run.work_id) covered.add(run.work_id);
    const ledger = run.work_id ? input.ledgers[run.work_id] : null;
    const [mode] = run.action.split(':');
    jobs.push({ key: `run:${run.run_id}`, workId: run.work_id, mode: ledger?.mode ?? (mode && !mode.includes(' ') ? mode : null), since: run.started_at,
      backend: ledger?.effective_backend ?? ledger?.backend ?? null, instance: null, model: ledger?.effective_model ?? null, action: run.action });
  }
  for (const claim of input.claims) {
    if (covered.has(claim.work_id)) continue;
    covered.add(claim.work_id);
    const ledger = input.ledgers[claim.work_id];
    jobs.push({ key: `claim:${claim.work_id}`, workId: claim.work_id, mode: ledger?.mode ?? claim.scope, since: claim.claimed_at,
      backend: ledger?.effective_backend ?? ledger?.backend ?? null, instance: null, model: ledger?.effective_model ?? null, action: null });
  }
  // Pair each job that names no backend with a factory agent process, oldest
  // with oldest: the loop starts a job's agent right after it claims the work.
  const processes = [...(input.factoryAgents ?? [])].sort((a, b) => (a.started_at ?? '').localeCompare(b.started_at ?? ''));
  const unnamed = jobs.filter((job) => !job.backend).sort((a, b) => (a.since ?? '').localeCompare(b.since ?? ''));
  unnamed.forEach((job, index) => { job.backend = processes[index]?.tool ?? null; job.model = job.model ?? processes[index]?.model ?? null; });
  const claimAge = (workId: string | null) => (workId ? input.claims.find((claim) => claim.work_id === workId)?.age_seconds ?? null : null);
  const state = (job: LiveJob): LiveState => (job.mode && GATE_MODES.has(job.mode) ? 'gates' : 'working');

  const used = new Set<string>();
  const rows: LiveAgentRow[] = [];
  for (const account of input.accounts) {
    const job = jobs.find((candidate) => !used.has(candidate.key)
      && (candidate.instance === account.id || (!candidate.instance || candidate.instance === candidate.backend) && candidate.backend === account.backend)) ?? null;
    if (job) used.add(job.key);
    rows.push({
      key: account.id,
      name: account.name,
      detail: account.backend,
      provider: account.provider,
      model: job?.model ?? account.model,
      state: job ? state(job) : !account.enabled ? 'halted' : account.notReady ? 'down' : account.resumes ? 'paused' : 'idle',
      job: job?.workId ?? null,
      mode: job?.mode ?? null,
      since: job?.since ?? null,
      claimAgeSeconds: claimAge(job?.workId ?? null),
      resumes: job ? null : account.resumes,
      reason: job ? null : account.notReady ?? account.pausedReason
    });
  }
  for (const job of jobs) {
    if (used.has(job.key)) continue;
    // A second job on an account that is already busy still belongs to that subscription.
    const account = input.accounts.find((candidate) => candidate.id === job.instance) ?? input.accounts.find((candidate) => candidate.backend === job.backend);
    rows.push({
      key: job.key,
      name: account?.name ?? job.instance ?? job.backend ?? 'controller',
      detail: account ? account.backend : job.action ?? job.backend,
      provider: account?.provider ?? null,
      model: job.model ?? account?.model ?? null,
      state: state(job),
      job: job.workId,
      mode: job.mode,
      since: job.since,
      claimAgeSeconds: claimAge(job.workId),
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
  const job = row.job ? <span className="font-mono text-primary">{row.job}</span> : 'a job';
  const elapsed = row.since ? <> for {formatDuration(now - Date.parse(row.since))}</> : null;
  let line: ReactNode;
  if (row.state === 'working') line = <>{row.mode ? `${row.mode} on ` : 'working on '}{job}{elapsed}</>;
  else if (row.state === 'gates') line = <>{row.mode ?? 'gates'} on {job}{elapsed}</>;
  else if (row.state === 'paused') line = <>paused on quota{row.reason ? ` (${row.reason})` : ''}, resumes in {formatDuration(Date.parse(row.resumes!) - now)} ({formatLocalTime(row.resumes) ?? row.resumes})</>;
  else if (row.state === 'halted') line = 'disabled, routing skips it';
  else if (row.state === 'down') line = <>not ready{row.reason ? `: ${row.reason}` : ''}</>;
  else line = 'idle, waiting for the router';
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
        <p className="truncate text-sm font-semibold text-primary" title={[row.name, row.model].filter(Boolean).join(' ')}>
          {agentDisplayName(row.name)}{row.model && <span className="font-normal text-secondary"> {capitalize(row.model)}</span>}
        </p>
        {row.detail && row.detail !== row.name && <p className="truncate text-[11px] text-muted" title={row.detail}>{row.detail}</p>}
      </div>
      <div className="min-w-0">
        <p className="text-sm tabular-nums text-secondary">{line}</p>
        <p className="truncate text-xs tabular-nums text-muted">
          {row.provider && <span className="rounded bg-raised px-1.5 py-0.5 text-[11px] text-secondary" title="Subscription">{row.provider}</span>}
          {row.provider && edits ? ' ' : ''}
          {edits}
        </p>
      </div>
    </li>
  );
}

/**
 * Live: what each agent account is doing now. Elapsed timers tick every
 * second while any row is busy; each busy job's latest ledger entry names
 * its backend and the files it changed. The footer is the controller's
 * latest run, so an idle fleet still says why.
 */
export function LiveAgentsCard({ profile, sessions, controllerRuns, claims, candidates, factoryAgents }: {
  /** Agent processes in factory worktrees, from the device scan. */
  factoryAgents?: DeviceAgent[];
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

  const accounts = useMemo(() => liveAccounts(instances ?? [], candidates, now),
    // `now` only sets the paused cut-off; the timer below re-renders the rows anyway.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [instances, candidates]);
  const rows = useMemo(() => buildLiveRows({ accounts, sessions, controllerRuns, claims, ledgers, factoryAgents }), [accounts, sessions, controllerRuns, claims, ledgers, factoryAgents]);
  const busyJobs = useMemo(() => [...new Set([
    ...sessions.filter((session) => ['starting', 'running', 'stopping'].includes(session.status)).map((session) => session.target),
    ...controllerRuns.filter((run) => run.status === 'running').map((run) => run.work_id),
    ...claims.map((claim) => claim.work_id)
  ].filter((id): id is string => !!id))].slice(0, 8), [sessions, controllerRuns, claims]);

  useEffect(() => {
    if (busyJobs.length === 0) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [busyJobs.length]);

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
  // Busy, paused and broken accounts need a look; idle subscriptions wait below in grey.
  const active = rows.filter((row) => row.state !== 'idle');
  const idle = rows.filter((row) => row.state === 'idle');
  const lastRun = controllerRuns.filter((run) => run.status !== 'running')
    .sort((a, b) => Date.parse(b.finished_at ?? b.started_at) - Date.parse(a.finished_at ?? a.started_at))[0];
  return (
    <section className="card-padded" aria-labelledby="live-agents-title">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h3 id="live-agents-title" className="flex items-center gap-2 text-sm font-semibold text-primary" title="Live: what each agent is doing now within the Factory">
          <Radio size={15} className="text-accent" aria-hidden="true" />
          Factory Agents Status
        </h3>
        <span className="text-xs tabular-nums text-muted">{busy > 0 ? `${busy} busy of ${rows.length}` : rows.length > 0 ? `${rows.length} accounts, none busy` : ''}</span>
      </div>
      {instancesError && <p role="alert" className="mb-2 text-xs text-critical">Cannot load agent accounts: {instancesError}</p>}
      {instances === null && !instancesError ? (
        <p className="text-sm text-muted">Loading agent accounts…</p>
      ) : rows.length === 0 ? (
        <p className="text-sm text-muted">No agent accounts configured for this profile and nothing running.</p>
      ) : (
        <>
          {active.length > 0 ? (
            <ul className="divide-y divide-subtle" aria-label="Agents">
              {active.map((row) => <LiveRow key={row.key} row={row} now={now} ledger={row.job ? ledgers[row.job] : null} />)}
            </ul>
          ) : (
            <p className="text-sm text-muted">Nothing is running; every subscription is idle.</p>
          )}
          {idle.length > 0 && (
            <div className="mt-3 border-t border-subtle pt-2 opacity-70">
              <h4 className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Idle subscriptions ({idle.length})</h4>
              <ul className="divide-y divide-subtle" aria-label="Idle subscriptions">
                {idle.map((row) => <LiveRow key={row.key} row={row} now={now} ledger={null} />)}
              </ul>
            </div>
          )}
        </>
      )}
      {lastRun && (
        <p className="mt-2 truncate text-xs text-muted" title={lastRun.outcome ?? lastRun.action}>
          Last run: {lastRun.work_id ? <span className="font-mono">{lastRun.work_id}</span> : null} {lastRun.action.split(':')[0]} · {lastRun.status}
          {lastRun.finished_at ? ` ${formatDuration(now - Date.parse(lastRun.finished_at))} ago` : ''}{lastRun.outcome ? ` · ${lastRun.outcome}` : ''}
        </p>
      )}
    </section>
  );
}
