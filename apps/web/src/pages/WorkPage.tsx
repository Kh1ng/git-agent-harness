import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, ChevronRight, ListChecks, FileText, Rocket } from 'lucide-react';
import type { AvailableTicket, ManagerBackendInfo, MergeRequest, ProviderKind, Session, UsageRollupTicketRow, WorkWaypointEvidence } from '@git-agent-harness/contracts';
import { generateProviderInstanceId } from '@git-agent-harness/shared';
import { gahApi, GahApiError } from '../api/client.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { LastUpdated } from '../components/ui/LastUpdated.js';
import { SessionCard } from '../components/SessionCard.js';
import { AttemptTimeline } from '../components/AttemptTimeline.js';
import { PaidRouteApprovals } from '../components/PaidRouteApprovals.js';
import { ExternalApprovals } from '../components/ExternalApprovals.js';
import { ControllerActivityCard } from '../components/ControllerActivityCard.js';
import { ProviderPicker } from '../components/ProviderPicker.js';
import { formatCost, formatPercent } from '../lib/format.js';
import {
  WAYPOINTS,
  WaypointHistory,
  WaypointStrip,
  currentWorkWaypoint,
  type WaypointEvidence
} from '../components/WaypointProgress.js';

const WORK_REFRESH_MS = 30 * 1000;

const DISPATCH_MODES = ['fix', 'improve', 'review', 'pm', 'experiment'] as const;

/** Minimal "start a new dispatch" form -- the dashboard could only stop/
 * command sessions that already existed, with no way to start one. Sends
 * the same `session.start` message the WS contract already defines
 * (apps/server's SessionManager.startSession); no new server-side work. */
function NewDispatchForm({ profile, repo, backends }: { profile: string; repo: string | null; backends: ManagerBackendInfo[] }) {
  const { sendMessage, isConnected } = useWebSocket();
  const [mode, setMode] = useState<(typeof DISPATCH_MODES)[number]>('fix');
  const [backend, setBackend] = useState<ProviderKind>('auto');
  const [target, setTarget] = useState('');
  const [justSent, setJustSent] = useState(false);

  const dispatch = () => {
    if (!repo) return;
    sendMessage({
      type: 'session.start',
      requestId: `dispatch_${Date.now()}`,
      profile,
      providerKind: backend,
      instanceId: generateProviderInstanceId(backend, 0),
      repo,
      mode,
      backend,
      target: target.trim() || undefined
    });
    setJustSent(true);
    setTimeout(() => setJustSent(false), 3000);
  };

  return (
    <section className="card-padded">
      <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
        <Rocket size={15} aria-hidden="true" />
        Queue work
      </h3>
      <div className="flex flex-wrap items-end gap-3">
        <label className="text-xs text-muted">
          Mode
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value as typeof mode)}
            className="block mt-1 bg-raised border border-subtle rounded-md px-2 py-1.5 text-sm text-primary"
          >
            {DISPATCH_MODES.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </label>
        <div className="text-xs text-muted">
          <span className="mb-1 block">Backend</span>
          <ProviderPicker
            backends={backends}
            selectedBackendId={backend}
            models={[]}
            currentModelId={null}
            reasoningEfforts={[]}
            currentReasoningEffortId={null}
            modelsLoaded
            busy={false}
            variant="backend"
            triggerAriaLabel="Factory backend"
            onSelect={(selection) => setBackend(selection.backendId as ProviderKind)}
          />
        </div>
        <label className="text-xs text-muted flex-1 min-w-[160px]">
          Target (issue number or ticket path)
          <input
            type="text"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="e.g. 148"
            className="block mt-1 w-full bg-raised border border-subtle rounded-md px-2 py-1.5 text-sm text-primary placeholder:text-muted"
          />
        </label>
        <button onClick={dispatch} disabled={!isConnected || !repo || !backends.find(item => item.id === backend)?.implemented} className="btn-primary">
          {justSent ? 'Sent' : 'Dispatch'}
        </button>
      </div>
      {!repo && <p className="text-xs text-critical mt-2">No repo known for this profile yet -- check Settings.</p>}
    </section>
  );
}

type WorkPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
};

type ProjectWorkItem = {
  key: string;
  workId: string | null;
  title: string;
  ticket: AvailableTicket | null;
  mergeRequest: MergeRequest | undefined;
  session: Session | undefined;
  ledgerEvidence: WorkWaypointEvidence | undefined;
};

function workKey(workId: string): string {
  const trimmed = workId.trim();
  if (/^\d+$/.test(trimmed)) return `#${Number(trimmed)}`;
  const issue = trimmed.match(/^#0*(\d+)$/);
  if (issue) return `#${Number(issue[1])}`;
  const ticket = trimmed.match(/^ticket-0*(\d+)$/i);
  if (ticket) return `#${Number(ticket[1])}`;
  return trimmed.toLowerCase();
}

function projectWorkItems(
  tickets: AvailableTicket[],
  mergeRequests: MergeRequest[],
  sessions: Session[],
  waypointEvidence: Record<string, WorkWaypointEvidence>
): ProjectWorkItem[] {
  const items = new Map<string, ProjectWorkItem>();
  const evidenceByKey = new Map(Object.entries(waypointEvidence).map(([workId, evidence]) => [workKey(workId), evidence]));
  for (const ticket of tickets) {
    const key = workKey(ticket.work_id ?? ticket.normalized_work_identity ?? ticket.ticket_path);
    items.set(key, {
      key,
      workId: ticket.work_id,
      title: ticket.title ?? ticket.work_id ?? ticket.ticket_path,
      ticket,
      mergeRequest: undefined,
      session: undefined,
      ledgerEvidence: evidenceByKey.get(key)
    });
  }
  for (const mergeRequest of mergeRequests) {
    if (!mergeRequest.work_id) continue;
    const key = workKey(mergeRequest.work_id);
    const existing = items.get(key);
    items.set(key, {
      key,
      workId: mergeRequest.work_id,
      title: existing?.title ?? mergeRequest.title ?? mergeRequest.work_id,
      ticket: existing?.ticket ?? null,
      mergeRequest,
      session: existing?.session,
      ledgerEvidence: existing?.ledgerEvidence ?? evidenceByKey.get(key)
    });
  }
  for (const session of sessions) {
    if (!session.target || !/^(?:#?\d+|ticket-\d+)$/i.test(session.target.trim())) continue;
    const key = workKey(session.target);
    const existing = items.get(key);
    items.set(key, {
      key,
      workId: existing?.workId ?? session.target,
      title: existing?.title ?? session.target,
      ticket: existing?.ticket ?? null,
      mergeRequest: existing?.mergeRequest,
      session,
      ledgerEvidence: existing?.ledgerEvidence ?? evidenceByKey.get(key)
    });
  }
  return [...items.values()];
}

function itemEvidence(item: ProjectWorkItem): WaypointEvidence {
  return {
    priorAttemptCount: item.ticket?.prior_attempt_count,
    hasActiveClaim: item.ticket?.has_active_claim,
    hasActiveMergeRequest: item.ticket?.has_active_mr,
    humanRequired: item.ticket?.human_required,
    sessionStatus: item.session?.status === 'idle' || item.session?.status === 'stopping' ? undefined : item.session?.status,
    sessionStartedAt: item.session?.startedAt,
    ledgerEvidence: item.ledgerEvidence,
    mergeRequest: item.mergeRequest
  };
}

function WorkDetail({ item, onBack }: { item: ProjectWorkItem; onBack: () => void }) {
  const workId = item.workId as string;
  const timeline = useGahStore((s) => s.workTimelines[workId]);
  const fetchWorkTimeline = useGahStore((s) => s.fetchWorkTimeline);

  useEffect(() => {
    fetchWorkTimeline(workId);
  }, [workId, fetchWorkTimeline]);

  const refresh = () => fetchWorkTimeline(workId, { force: true });
  useAutoRefresh(refresh, WORK_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  return (
    <div>
      <button onClick={onBack} className="inline-flex items-center gap-1.5 text-sm text-secondary hover:text-primary mb-4">
        <ArrowLeft size={15} aria-hidden="true" />
        Back to work list
      </button>
      <div className="flex items-start justify-between gap-4 mb-1">
        <h2 className="text-lg font-semibold text-primary">{workId}</h2>
        <LastUpdated at={timeline?.fetchedAt ?? null} />
      </div>
      <p className="text-sm text-muted mb-5">
        Dispatch → attempt → fallback → validation → repair → review → merge, in order.
      </p>

      {timeline?.loading && !timeline.data ? (
        <LoadingState label="Loading attempt history…" />
      ) : timeline?.error ? (
        <ErrorState
          message={timeline.error}
          endpoint={`/api/work/${workId}`}
          onRetry={() => fetchWorkTimeline(workId, { force: true })}
        />
      ) : !timeline?.data || timeline.data.length === 0 ? (
        <div className="space-y-4">
          <WaypointHistory evidence={itemEvidence(item)} label={workId} />
          <EmptyState icon={FileText} title="No ledger history for this work item yet" />
        </div>
      ) : (
        <div className="space-y-4">
          <WaypointHistory evidence={{ ...itemEvidence(item), entries: timeline.data }} label={workId} />
          <AttemptTimeline entries={timeline.data} />
        </div>
      )}
    </div>
  );
}

export function WorkPage({ sessions, onSelectSession }: WorkPageProps) {
  const { profile: wsProfile, controllerActivity } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const status = useGahStore((s) => s.status);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const report = useGahStore((s) => s.report);
  const fetchReport = useGahStore((s) => s.fetchReport);
  const [selectedWorkId, setSelectedWorkId] = useState<string | null>(null);
  const [holdPending, setHoldPending] = useState<string | null>(null);
  const [holdError, setHoldError] = useState<string | null>(null);
  const [factoryData, setFactoryData] = useState<{ profile: string | null; usageTickets: UsageRollupTicketRow[]; managerBackends: ManagerBackendInfo[] }>({
    profile: null,
    usageTickets: [],
    managerBackends: []
  });
  const [paidReviewWorkIds, setPaidReviewWorkIds] = useState<string[]>([]);
  const [externalReviewWorkIds, setExternalReviewWorkIds] = useState<string[]>([]);
  const factoryRequestId = useRef(0);
  const activeProfileRef = useRef(profile);
  activeProfileRef.current = profile;

  const refreshFactoryData = async () => {
    const requestedProfile = profile ?? null;
    const requestId = ++factoryRequestId.current;
    setFactoryData(current => current.profile === requestedProfile
      ? current
      : { profile: requestedProfile, usageTickets: [], managerBackends: [] });
    const [usage, settings] = await Promise.allSettled([
      gahApi.getUsageRollup(profile ?? undefined, 30),
      gahApi.getManagerChatSettings()
    ]);
    if (factoryRequestId.current !== requestId) return;
    setFactoryData({
      profile: requestedProfile,
      usageTickets: usage.status === 'fulfilled' ? usage.value.tickets : [],
      managerBackends: settings.status === 'fulfilled' ? settings.value.availableBackends : []
    });
  };

  const refresh = async () => {
    await Promise.all([
      fetchStatus(profile ?? undefined, { force: true }),
      fetchProfiles({ force: true }),
      fetchReport({ profile: profile ?? undefined, since: '7d', groupBy: 'backend' }, { force: true }),
      refreshFactoryData()
    ]);
  };

  useEffect(() => {
    setPaidReviewWorkIds([]);
    setExternalReviewWorkIds([]);
    void refresh();
    return () => { factoryRequestId.current += 1; };
    // Each fetch function is a stable Zustand action; refresh is intentionally
    // recreated with the selected profile.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, fetchStatus, fetchProfiles, fetchReport]);

  useAutoRefresh(() => { void refresh(); }, WORK_REFRESH_MS);
  useWsReconnectRefresh(() => { void refresh(); });

  const activeProfileRepo = profiles.data?.find((p) => p.name === profile)?.repo ?? null;

  const projectItems = useMemo(
    () => projectWorkItems(
      status.data?.available_tickets ?? [],
      status.data?.merge_requests ?? [],
      sessions,
      status.data?.work_waypoint_evidence ?? {}
    ),
    [status.data?.available_tickets, status.data?.merge_requests, status.data?.work_waypoint_evidence, sessions]
  );

  if (selectedWorkId) {
    const selectedItem = projectItems.find((item) => item.workId === selectedWorkId);
    if (selectedItem) return <WorkDetail item={selectedItem} onBack={() => setSelectedWorkId(null)} />;
  }

  const activeSessions = sessions.filter((s) => s.status === 'running');
  const queuedSessions = sessions.filter((s) => s.status === 'starting');
  const recentSessions = sessions.filter((s) => ['stopped', 'error'].includes(s.status)).slice(0, 5);
  const issueIntakeRejections = status.data?.issue_intake_rejections ?? [];
  const activeClaims = status.data?.active_claims ?? [];
  const heldWorkIds = new Set(status.data?.review_held_work_ids ?? []);
  const usageByWorkKey = new Map(factoryData.usageTickets.map((row) => [workKey(row.ticket), row]));
  const waypointCounts = new Map(WAYPOINTS.map((waypoint) => [waypoint.key, 0]));
  for (const item of projectItems) {
    const waypoint = currentWorkWaypoint(itemEvidence(item));
    waypointCounts.set(waypoint.key, (waypointCounts.get(waypoint.key) ?? 0) + 1);
  }
  const sessionWorkKey = (session: Session) => session.target ? workKey(session.target) : `session:${session.id}`;
  const inactiveSessionKeys = new Set(recentSessions.map(sessionWorkKey));
  const queuedWork = new Set(queuedSessions.map(sessionWorkKey));
  for (const ticket of status.data?.available_tickets ?? []) {
    const key = workKey(ticket.work_id ?? ticket.normalized_work_identity ?? ticket.ticket_path);
    if (ticket.execution_policy?.dispatchable_now && ticket.prior_attempt_count === 0 && !ticket.has_active_claim && !ticket.has_active_mr && !ticket.human_required && !inactiveSessionKeys.has(key)) queuedWork.add(key);
  }
  const runningWork = new Set(activeSessions.map(sessionWorkKey));
  for (const claim of activeClaims) runningWork.add(workKey(claim.work_id));
  const reviewWork = new Map<string, { workId: string; title: string | null; url: string | null; reasons: Set<string> }>();
  const addReviewWork = (workId: string, reason: string, title: string | null = null, url: string | null = null) => {
    const key = workKey(workId);
    const existing = reviewWork.get(key);
    if (existing) {
      existing.reasons.add(reason);
      if (!existing.title && title) existing.title = title;
      if (!existing.url && url) existing.url = url;
    } else reviewWork.set(key, { workId, title, url, reasons: new Set([reason]) });
  };
  for (const workId of heldWorkIds) addReviewWork(workId, 'Review hold');
  for (const ticket of status.data?.available_tickets ?? []) if (ticket.human_required) addReviewWork(ticket.work_id ?? ticket.normalized_work_identity ?? ticket.ticket_path, 'Human required', ticket.title);
  for (const mergeRequest of status.data?.merge_requests ?? []) if (!mergeRequest.merged && mergeRequest.work_id) addReviewWork(mergeRequest.work_id, 'Pull request', mergeRequest.title ?? null, mergeRequest.url);
  for (const blocker of status.data?.blocked_work_items ?? []) {
    const workId = blocker.remediation_plan?.work_id ?? blocker.source_reference;
    if (workId) addReviewWork(workId, blocker.reason_code?.replace(/_/g, ' ') ?? 'Blocked');
  }
  for (const workId of paidReviewWorkIds) addReviewWork(workId, 'Paid route approval');
  for (const workId of externalReviewWorkIds) addReviewWork(workId, 'External API approval');
  const shippedWork = new Set((status.data?.merge_requests ?? []).filter(mergeRequest => mergeRequest.merged).map(mergeRequest => workKey(mergeRequest.work_id ?? mergeRequest.id ?? mergeRequest.branch)));
  const factoryCounts = {
    queued: queuedWork.size,
    running: runningWork.size,
    review: reviewWork.size,
    shipped: shippedWork.size
  };
  const dispatchBackends: ManagerBackendInfo[] = [
    { id: 'auto', displayName: 'Auto', implemented: factoryData.managerBackends.some(item => item.implemented && status.data?.backend_configured?.[item.id] === true) },
    ...factoryData.managerBackends.map(item => ({ ...item, implemented: item.implemented && status.data?.backend_configured?.[item.id] === true }))
  ];
  const reportKey = JSON.stringify({ profile: profile ?? undefined, since: '7d', groupBy: 'backend' });
  const currentReport = report.key === reportKey && !report.error ? report.data : null;
  const reportEntries = currentReport?.comparisons.reduce((sum, row) => sum + row.entries, 0) ?? 0;
  const reportSuccesses = currentReport?.comparisons.reduce((sum, row) => sum + row.validation_pass, 0) ?? 0;
  const successRate = reportEntries > 0 ? reportSuccesses / reportEntries : null;
  // Issue #503: hold/release safe controls — the same authorization the CLI
  // enforces, via the owner-gated mutation API.
  const setHold = async (workId: string, reason: string) => {
    setHoldPending(workId);
    setHoldError(null);
    try {
      await gahApi.holdSet({ profile: profile ?? 'gah', work_id: workId, reason });
      await refresh();
    } catch (failure) {
      setHoldError(failure instanceof GahApiError ? failure.message : 'Hold failed.');
    } finally {
      setHoldPending(null);
    }
  };
  const clearHold = async (workId: string) => {
    setHoldPending(workId);
    setHoldError(null);
    try {
      await gahApi.holdClear({ profile: profile ?? 'gah', work_id: workId });
      await refresh();
    } catch (failure) {
      setHoldError(failure instanceof GahApiError ? failure.message : 'Release failed.');
    } finally {
      setHoldPending(null);
    }
  };

  const formatClaimAge = (ageSeconds: number): string => {
    const minutes = Math.floor(ageSeconds / 60);
    const seconds = ageSeconds % 60;
    if (minutes > 0) {
      return `${minutes}m ${seconds}s`;
    }
    return `${seconds}s`;
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Factory"
        description="Queue, run, review, and ship agent work"
        onRefresh={refresh}
        refreshing={status.loading}
        lastUpdated={status.fetchedAt}
      />

      <section aria-labelledby="factory-flow-title">
        <h3 id="factory-flow-title" className="sr-only">Factory flow</h3>
        <dl className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
          {[
            ['Queued', factoryCounts.queued],
            ['Running', factoryCounts.running],
            ['Needs review', factoryCounts.review],
            ['Shipped', factoryCounts.shipped]
          ].map(([label, value]) => (
            <div key={label} className="card-padded">
              <dt className="text-xs text-muted">{label}</dt>
              <dd className="mt-1 text-xl font-semibold tabular-nums text-primary">{value}</dd>
            </div>
          ))}
          <div className="card-padded col-span-2 sm:col-span-1">
            <dt className="text-xs text-muted">Capacity</dt>
            <dd className="mt-1 text-xl font-semibold tabular-nums text-primary">
              {status.data?.inflight_implementation_count ?? activeSessions.length}/{status.data?.max_parallel_workers ?? '—'}
            </dd>
          </div>
          <div className="card-padded">
            <dt className="text-xs text-muted">Success · 7d</dt>
            <dd className="mt-1 text-xl font-semibold tabular-nums text-primary">{formatPercent(successRate)}</dd>
          </div>
        </dl>
      </section>

      <NewDispatchForm profile={profile ?? 'gah'} repo={activeProfileRepo} backends={dispatchBackends} />

      {profile && (
        <div aria-labelledby="factory-review-title">
          <h3 id="factory-review-title" className="mb-3 text-sm font-semibold text-primary">Needs review ({factoryCounts.review})</h3>
          <div className="space-y-3">
            {reviewWork.size > 0 && (
              <ul className="card divide-y divide-subtle" aria-label="Review queue">
                {[...reviewWork.values()].map(item => (
                  <li key={workKey(item.workId)} className="flex flex-wrap items-center justify-between gap-3 px-3 py-3">
                    <div className="min-w-0">
                      <p className="text-sm text-primary"><span className="font-mono text-xs text-muted">{item.workId}</span>{item.title ? ` ${item.title}` : ''}</p>
                      <div className="mt-1 flex flex-wrap gap-1">
                        {[...item.reasons].map(reason => <StatusBadge key={reason} tone="warning" label={reason} />)}
                      </div>
                    </div>
                    <div className="flex items-center gap-2">
                      {item.url && <a href={item.url} target="_blank" rel="noreferrer" className="text-xs text-accent hover:underline">Open PR</a>}
                      {item.reasons.has('Review hold') && (
                        <button type="button" disabled={holdPending !== null} onClick={() => clearHold(item.workId)} className="btn-secondary min-h-11 sm:min-h-0 sm:py-1">
                          {holdPending === item.workId ? 'Releasing…' : 'Release hold'}
                        </button>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
            <PaidRouteApprovals
              key={profile}
              profile={profile}
              onPendingWorkIds={(workIds) => { if (activeProfileRef.current === profile) setPaidReviewWorkIds(workIds); }}
            />
            <ExternalApprovals
              key={`external-${profile}`}
              profile={profile}
              onPendingWorkIds={(workIds) => { if (activeProfileRef.current === profile) setExternalReviewWorkIds(workIds); }}
            />
          </div>
        </div>
      )}

      <section>
        <ControllerActivityCard activity={controllerActivity} />
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Running sessions ({activeSessions.length})</h3>
        {activeSessions.length === 0 ? (
          <EmptyState icon={ListChecks} title="No running sessions" />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {activeSessions.map((session) => (
              <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
            ))}
          </div>
        )}
      </section>

      {queuedSessions.length > 0 && (
        <section>
          <details className="group">
            <summary className="mb-3 flex cursor-pointer list-none items-center gap-2 text-sm font-semibold text-secondary hover:text-primary [&::-webkit-details-marker]:hidden">
              <ChevronRight size={14} className="transition-transform group-open:rotate-90" aria-hidden="true" />
              Queued sessions ({queuedSessions.length})
            </summary>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {queuedSessions.map((session) => (
                <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
              ))}
            </div>
          </details>
        </section>
      )}

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">
          Active durable claims ({activeClaims.length})
        </h3>
        {activeClaims.length === 0 ? (
          <EmptyState icon={ListChecks} title="No active claims" description="No in-flight claims are being tracked." />
        ) : (
          <div className="card overflow-x-auto">
            <table className="table-base min-w-[720px]">
              <thead>
                <tr>
                  <th>Work ID</th>
                  <th>Owner PID / Scope</th>
                  <th>Hostname</th>
                  <th>Claim age</th>
                </tr>
              </thead>
              <tbody>
                {activeClaims.map((claim) => (
                  <tr key={`${claim.scope}-${claim.work_id}-${claim.pid}`}>
                    <td className="text-primary font-mono text-xs">{claim.work_id}</td>
                    <td>
                      <span className="font-mono text-xs text-muted mr-2">{claim.pid}</span>
                      <span className="font-mono text-xs">{claim.scope}</span>
                    </td>
                    <td>{claim.hostname}</td>
                    <td>{formatClaimAge(claim.age_seconds)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {recentSessions.length > 0 && (
        <section>
          <h3 className="text-sm font-semibold text-primary mb-3">Recent outcomes</h3>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {recentSessions.map((session) => (
              <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
            ))}
          </div>
        </section>
      )}

      <section>
        <div className="mb-3 flex flex-wrap items-end justify-between gap-2">
          <div>
            <h3 className="text-sm font-semibold text-primary">Factory pipeline ({projectItems.length})</h3>
            <p className="mt-1 text-xs text-muted">Current stage by ticket, derived from the queue, sessions, ledger, and provider state.</p>
          </div>
        </div>
        {status.loading && !status.data ? (
          <LoadingState label="Loading tickets…" />
        ) : status.error ? (
          <ErrorState
            message={status.error}
            endpoint="/api/status"
            onRetry={() => fetchStatus(profile ?? undefined, { force: true })}
          />
        ) : projectItems.length === 0 ? (
          <EmptyState icon={ListChecks} title="No tracked work" description="No ticket, session, or pull request is currently visible for this profile." />
        ) : (
          <div className="card overflow-hidden">
            <dl className="grid grid-cols-3 border-b border-subtle sm:grid-cols-6">
              {WAYPOINTS.map((waypoint) => (
                <div key={waypoint.key} className="px-3 py-2.5 text-center">
                  <dt className="text-[11px] text-muted">{waypoint.label}</dt>
                  <dd className="mt-0.5 text-sm font-semibold tabular-nums text-primary">{waypointCounts.get(waypoint.key) ?? 0}</dd>
                </div>
              ))}
            </dl>
            <div className="overflow-x-auto">
            <table className="table-base min-w-[640px] sm:min-w-[900px]">
              <thead>
                <tr>
                  <th>Work item</th>
                  <th>Waypoints</th>
                  <th>Backend / model</th>
                  <th>Attempts</th>
                  <th>Cost · 30d</th>
                  <th>Status</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {projectItems.map((item) => {
                  const t = item.ticket;
                  const evidence = itemEvidence(item);
                  const currentWaypoint = currentWorkWaypoint(evidence);
                  return (
                  <tr key={item.key}>
                    <td className="text-primary">
                      {item.workId && <span className="font-mono text-xs text-muted mr-1.5">{item.workId}</span>}
                      {item.title !== item.workId ? item.title : null}
                    </td>
                    <td>
                      <div className="max-w-[470px] overflow-x-auto pb-1">
                        <WaypointStrip evidence={evidence} label={item.workId ?? item.title} />
                      </div>
                    </td>
                    <td>
                      {t?.recommended_backend
                        ? `${t.recommended_backend}${t.recommended_model ? `/${t.recommended_model}` : ''}`
                        : item.session?.backend
                          ? `${item.session.backend}${item.session.model ? `/${item.session.model}` : ''}`
                        : 'Unknown'}
                    </td>
                    <td>{t?.prior_attempt_count ?? '—'}</td>
                    <td>{usageByWorkKey.get(item.key)?.estimated_cost_usd == null ? <span className="text-muted">Unmeasured</span> : formatCost(usageByWorkKey.get(item.key)?.estimated_cost_usd)}</td>
                    <td>
                      <div className="flex flex-wrap gap-1">
                        <StatusBadge tone={evidence.humanRequired ? 'warning' : currentWaypoint.key === 'merged' ? 'good' : 'unknown'} label={currentWaypoint.label} />
                        {item.workId && heldWorkIds.has(item.workId) && (
                          <StatusBadge tone="warning" label="Review hold" />
                        )}
                        {t?.work_id && (
                          heldWorkIds.has(t.work_id) ? (
                            <button
                              type="button"
                              disabled={holdPending !== null}
                              onClick={() => clearHold(t.work_id as string)}
                              className="px-2 py-0.5 border border-subtle rounded text-xs text-secondary hover:text-primary"
                            >
                              {holdPending === t.work_id ? '…' : 'Release'}
                            </button>
                          ) : (
                            <button
                              type="button"
                              disabled={holdPending !== null}
                              onClick={() => setHold(t.work_id as string, 'operator review hold from dashboard')}
                              className="px-2 py-0.5 border border-subtle rounded text-xs text-secondary hover:text-primary"
                            >
                              {holdPending === t.work_id ? '…' : 'Hold'}
                            </button>
                          )
                        )}
                        {t?.human_required ? (
                          <StatusBadge tone="warning" label="Human required" />
                        ) : t?.has_active_claim ? (
                          <StatusBadge tone="warning" label="Claimed" />
                        ) : item.mergeRequest?.merged ? (
                          <StatusBadge tone="good" label="Merged" />
                        ) : t?.has_active_mr || item.mergeRequest ? (
                          <StatusBadge tone="good" label="Active MR" />
                        ) : (t?.prior_attempt_count ?? 0) > 0 ? (
                          <StatusBadge tone="serious" label={t?.last_failure_class ?? 'Retrying'} />
                        ) : (
                          <StatusBadge tone="unknown" label="Not dispatched" />
                        )}
                      </div>
                    </td>
                    <td>
                      {item.workId && (
                        <button
                          onClick={() => setSelectedWorkId(item.workId)}
                          className="inline-flex min-h-11 items-center text-xs text-accent underline-offset-4 hover:underline focus-visible:underline sm:min-h-0 sm:py-1"
                        >
                          View history
                        </button>
                      )}
                    </td>
                  </tr>
                );})}
              </tbody>
            </table>
            </div>
          </div>
        )}
        {holdError && <p role="alert" className="mt-2 text-xs text-critical">{holdError}</p>}
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">
          Issue intake rejections ({issueIntakeRejections.length})
        </h3>
        {status.loading && !status.data ? (
          <LoadingState label="Loading intake policy…" />
        ) : issueIntakeRejections.length === 0 ? (
          <EmptyState
            icon={ListChecks}
            title="No issue intake rejections"
            description="Every discovered issue currently satisfies the intake policy."
          />
        ) : (
          <div className="card overflow-x-auto">
            <table className="table-base min-w-[720px]">
              <thead>
                <tr>
                  <th>Issue</th>
                  <th>Author</th>
                  <th>Disposition</th>
                  <th>Reason</th>
                </tr>
              </thead>
              <tbody>
                {issueIntakeRejections.map((rejection) => (
                  <tr key={`${rejection.provider}-${rejection.ticket_path}`}>
                    <td className="text-primary">
                      {rejection.work_id && (
                        <span className="font-mono text-xs text-muted mr-1.5">{rejection.work_id}</span>
                      )}
                      {rejection.title ?? rejection.ticket_path}
                    </td>
                    <td>
                      <span className="text-secondary">
                        {rejection.author_login ?? 'unknown'}
                      </span>
                      {rejection.author_kind && (
                        <span className="ml-2 text-xs text-muted font-mono">
                          {rejection.author_kind}
                        </span>
                      )}
                    </td>
                    <td>
                      <StatusBadge tone="warning" label={rejection.reason_code} />
                    </td>
                    <td className="text-sm text-muted">{rejection.reason}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
