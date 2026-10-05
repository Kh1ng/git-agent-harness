import { useEffect } from 'react';
import { ExternalAnchor } from '../components/ExternalAnchor';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import {
  ListChecks,
  CheckCircle2,
  Coins,
  Timer,
  GitMerge,
  AlertTriangle,
  Play,
  Square
} from 'lucide-react';
import type { Page } from '../App.js';
import type { DeviceAgentsSnapshot, Session } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { StatTile } from '../components/ui/StatTile.js';
import { StatusBadge, classificationTone } from '../components/ui/StatusBadge.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { formatPercent, formatAge, formatLocalTime, isStale, formatTokens, formatCount, oldestFetchedAt } from '../lib/format.js';
import { AttentionTable, attentionRows } from '../components/AttentionTable.js';
import { LiveAgentsCard, liveRowTitle } from '../components/LiveAgentsCard.js';
import type { WatchableRun } from '../components/AgentLiveView.js';
import { NonFactoryAgentsCard } from '../components/NonFactoryAgentsCard.js';

type OverviewPageProps = {
  sessions: Session[];
  onNavigate: (page: Page) => void;
  onOpenWork?: (workId: string) => void;
  /** Opens a running job's read-only live view. */
  onWatchRun?: (run: WatchableRun, running: WatchableRun[]) => void;
  /** The device's agent processes; absent in isolation (component tests). */
  deviceAgents?: { data: DeviceAgentsSnapshot | null; error: string | null };
};

const OVERVIEW_REFRESH_MS = 5 * 60 * 1000;

export function OverviewPage({ sessions, onNavigate, onOpenWork = () => {}, onWatchRun, deviceAgents = { data: null, error: null } }: OverviewPageProps) {
  const { status, quota, loopStatus, loopAction } = useGahStore();
  const { profile: wsProfile, controllerActivity } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
  const openChatSession = useUiStore((s) => s.openChatSession);
  const profile = profileOverride ?? wsProfile;
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const fetchQuota = useGahStore((s) => s.fetchQuota);
  const fetchLoopStatus = useGahStore((s) => s.fetchLoopStatus);
  const startLoop = useGahStore((s) => s.startLoop);
  const stopLoop = useGahStore((s) => s.stopLoop);

  useEffect(() => {
    fetchStatus(profile ?? undefined);
    fetchQuota({ profile: profile ?? undefined, since: '7d' });
    if (profile) fetchLoopStatus(profile);
  }, [profile, fetchStatus, fetchQuota, fetchLoopStatus]);

  const refresh = () => {
    fetchStatus(profile ?? undefined, { force: true });
    fetchQuota({ profile: profile ?? undefined, since: '7d' }, { force: true });
  };
  useAutoRefresh(refresh, OVERVIEW_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const loopRunning = loopStatus.data?.running ?? false;
  const toggleLoop = () => {
    if (!profile) return;
    if (loopRunning) {
      stopLoop(profile);
    } else {
      startLoop(profile);
    }
  };

  const activeSessions = sessions.filter((s) => ['starting', 'running'].includes(s.status));
  const activeControllerRuns = controllerActivity.filter((run) => run.status === 'running');
  const activeWorkCount = activeSessions.length + activeControllerRuns.length;
  const lastUpdated = oldestFetchedAt(status.fetchedAt, quota.fetchedAt);

  // The page's own title/refresh control renders unconditionally below --
  // only the content area swaps to loading/error, so a total data-fetch
  // failure never leaves the user looking at a page with no identity and
  // no way to retry.
  if ((status.loading && !status.data) || (quota.loading && !quota.data)) {
    return (
      <div className="space-y-6">
        <PageHeader title="Overview" onRefresh={refresh} refreshing lastUpdated={lastUpdated} />
        <LoadingState label="Loading status…" />
      </div>
    );
  }
  if ((status.error && !status.data) || (quota.error && !quota.data)) {
    return (
      <div className="space-y-6">
        <PageHeader title="Overview" onRefresh={refresh} lastUpdated={lastUpdated} />
        <ErrorState message={status.error ?? quota.error ?? 'Failed to load overview data'} endpoint="/api/status" onRetry={refresh} />
      </div>
    );
  }

  const snapshot = status.data;
  const quotaSnapshot = quota.data;
  // Genuine profile-wide blockers (sync down, no viable route) vs.
  // work-item-scoped blockers (a ticket/MR needs a human) are two
  // different fields -- a ticket being blocked does NOT freeze the whole
  // profile, so `blockers` is often empty even when `blocked_work_items`
  // has entries. See status.rs's own doc comments on StatusSnapshot.
  const blockers = snapshot?.blockers ?? [];
  const blockedWorkItems = snapshot?.blocked_work_items ?? [];
  const reviewHeldWorkIds = snapshot?.review_held_work_ids ?? [];
  // Native issue prerequisites that block autonomous intake.
  const dependencyBlockers = snapshot?.dependency_blockers ?? [];
  const attention = attentionRows({ blockers, dependencyBlockers, blockedWorkItems, reviewHeldWorkIds });
  const needsReviewMrs = (snapshot?.merge_requests ?? []).filter((m) => m.classification === 'NEEDS_REVIEW');
  const recentMerges = (snapshot?.merge_requests ?? []).filter((m) => m.classification === 'MERGED').slice(0, 5);
  const unavailableBackends = (quotaSnapshot?.candidates ?? []).filter((c) => !c.eligible_now);
  const usage = quotaSnapshot?.usage;
  const totalEntries = usage?.entries ?? 0;
  const successRate = usage?.success_rate ?? null;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Overview"
        description={snapshot ? `Profile: ${snapshot.profile.display_name}` : undefined}
        onRefresh={refresh}
        refreshing={status.loading}
        lastUpdated={lastUpdated}
        actions={
          profile && (
            <div className="flex items-center gap-2">
              <StatusBadge tone={loopRunning ? 'good' : 'unknown'} label={loopRunning ? 'Loop running' : 'Loop stopped'} />
              <button
                onClick={toggleLoop}
                disabled={loopAction.pending || loopStatus.loading}
                className={loopRunning ? 'btn-secondary text-critical border-critical/30' : 'btn-secondary'}
                title={loopAction.error ?? undefined}
              >
                {loopRunning ? <Square size={14} aria-hidden="true" /> : <Play size={14} aria-hidden="true" />}
                <span className="hidden sm:inline">{loopRunning ? 'Stop loop' : 'Start loop'}</span>
              </button>
            </div>
          )
        }
      />
      {loopAction.error && (
        <p className="text-xs text-critical -mt-4">{loopAction.error}</p>
      )}

      <div className="grid grid-cols-2 lg:grid-cols-5 gap-3">
        <StatTile label="Tasks (7d)" value={formatCount(usage?.entries)} icon={ListChecks} />
        <StatTile
          label="Success rate"
          value={formatPercent(successRate)}
          icon={CheckCircle2}
          hint={usage?.entries !== null && usage?.entries !== undefined ? `${usage?.validation_pass ?? 0}/${totalEntries} validated` : undefined}
        />
        <StatTile
          label="Usage (7d)"
          value={formatTokens(usage?.total_tokens)}
          icon={Coins}
          hint={usage?.requests_count !== null && usage?.requests_count !== undefined ? `${formatCount(usage.requests_count)} requests` : undefined}
        />
        <StatTile label="Active work" value={String(activeWorkCount)} icon={Timer} hint={`${activeSessions.length} dashboard · ${activeControllerRuns.length} controller`} />
        {/* The full candidate list lives on Usage > Quota; the tile only says whether routing is constrained. */}
        <button type="button" onClick={() => onNavigate('quota')} className="text-left" aria-label="Backend availability: open Quota">
          <StatTile label="Backends" icon={CheckCircle2}
            value={(quotaSnapshot?.candidates.length ?? 0) === 0 ? '—' : unavailableBackends.length === 0 ? 'All eligible' : `${unavailableBackends.length} down`}
            hint={(quotaSnapshot?.candidates.length ?? 0) === 0 ? 'No quota snapshot' : `${quotaSnapshot!.candidates.length} candidates`} />
        </button>
      </div>

      <LiveAgentsCard profile={profile ?? null} sessions={sessions} controllerRuns={controllerActivity}
        claims={snapshot?.active_claims ?? []} candidates={quotaSnapshot?.candidates ?? []} factoryAgents={deviceAgents.data?.factory_agents}
        onWatch={onWatchRun ? (row, watchable) => {
          const asRun = (item: typeof row): WatchableRun => ({ runId: item.runId!, title: liveRowTitle(item), subtitle: item.mode });
          if (row.runId) onWatchRun(asRun(row), watchable.map(asRun));
        } : undefined} />

      <NonFactoryAgentsCard device={deviceAgents.data} deviceError={deviceAgents.error} onOpenChat={(chatProfile, sessionId) => { openChatSession(chatProfile, sessionId); onNavigate('chat'); }} />

      {attention.length > 0 && <AttentionTable rows={attention} onOpenWork={onOpenWork} />}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <section>
          <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
            <AlertTriangle size={15} className="text-warning" aria-hidden="true" />
            Needs review ({needsReviewMrs.length})
          </h3>
          {needsReviewMrs.length === 0 ? (
            <EmptyState icon={CheckCircle2} title="Nothing awaiting review" />
          ) : (
            <div className="card overflow-hidden">
              <table className="table-base">
                <tbody>
                  {needsReviewMrs.map((mr) => {
                    const { tone, label } = classificationTone(mr.classification);
                    return (
                      <tr key={mr.branch} className={mr.work_id ? 'cursor-pointer hover:bg-raised/50' : undefined} onClick={() => mr.work_id && onOpenWork(mr.work_id)}>
                        <td className="font-mono text-xs">{mr.branch}</td>
                        <td>
                          <StatusBadge tone={tone} label={label} />
                        </td>
                        <td>
                          <div className="flex items-center gap-3">
                            {mr.work_id && <button type="button" onClick={(event) => { event.stopPropagation(); onOpenWork(mr.work_id as string); }} className="min-h-11 text-xs text-accent hover:underline sm:min-h-0">View details</button>}
                            {/* Rows without a work id (recent merges) still reach the provider. */}
                            {mr.url && (
                              <ExternalAnchor href={mr.url} className="min-h-11 text-xs text-accent hover:underline sm:min-h-0">
                                View MR
                              </ExternalAnchor>
                            )}
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section>
          <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
            <GitMerge size={15} className="text-good" aria-hidden="true" />
            Recently merged
          </h3>
          {recentMerges.length === 0 ? (
            <EmptyState icon={GitMerge} title="No recent merges" />
          ) : (
            <div className="card overflow-hidden">
              <table className="table-base">
                <thead>
                  <tr>
                    <th>Work</th>
                    <th>Title</th>
                    <th>Backend</th>
                    <th>Merged</th>
                    <th>Review</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {recentMerges.map((mr) => (
                    <tr key={mr.branch} className={mr.work_id ? 'cursor-pointer hover:bg-raised/50' : undefined} onClick={() => mr.work_id && onOpenWork(mr.work_id)}>
                      <td className="font-mono text-xs whitespace-nowrap">
                        {mr.work_id ?? <span className="text-muted">—</span>}
                      </td>
                      <td className="text-xs max-w-[16rem] truncate" title={mr.title ?? mr.branch}>
                        {mr.url ? (
                          <ExternalAnchor href={mr.url} className="text-primary hover:text-accent hover:underline">
                            {mr.title ?? mr.branch}
                          </ExternalAnchor>
                        ) : (
                          (mr.title ?? mr.branch)
                        )}
                      </td>
                      <td className="text-xs whitespace-nowrap text-secondary">
                        {mr.effective_backend
                          ? `${mr.effective_backend}${mr.effective_model ? `/${mr.effective_model}` : ''}`
                          : <span className="text-muted">—</span>}
                      </td>
                      <td className="text-xs whitespace-nowrap text-secondary">
                        {mr.merged_at ? (formatAge(mr.merged_at) ?? formatLocalTime(mr.merged_at) ?? '—') : <span className="text-muted">—</span>}
                      </td>
                      <td className="text-xs whitespace-nowrap">
                        {mr.review_verdict || mr.review_gate_reason ? (
                          <div className="space-y-1">
                            <StatusBadge
                              tone={mr.review_verdict?.toLowerCase().includes('approve') ? 'good' : 'warning'}
                              label={mr.review_verdict ?? 'HUMAN_REVIEW'}
                            />
                            {mr.review_gate_reason && (
                              <p className="max-w-48 truncate text-[10px] text-warning" title={mr.review_gate_reason}>
                                {mr.review_gate_reason}
                              </p>
                            )}
                          </div>
                        ) : (
                          <span className="text-muted">—</span>
                        )}
                      </td>
                      <td>
                        <div className="flex items-center gap-3">
                          {mr.work_id && <button type="button" onClick={(event) => { event.stopPropagation(); onOpenWork(mr.work_id as string); }} className="min-h-11 text-xs text-accent hover:underline sm:min-h-0">View details</button>}
                          {mr.url && (
                            <ExternalAnchor href={mr.url} className="min-h-11 text-xs text-accent hover:underline sm:min-h-0">
                              View
                            </ExternalAnchor>
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      </div>

      {snapshot?.recent_ledger && (
        <section className="card-padded">
          <h3 className="text-sm font-semibold text-primary mb-3">Most recent dispatch</h3>
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 text-sm">
            <div>
              <p className="text-xs text-muted uppercase tracking-wide mb-1">Mode</p>
              <p className="text-primary">{snapshot.recent_ledger.most_recent_mode}</p>
            </div>
            <div>
              <p className="text-xs text-muted uppercase tracking-wide mb-1">Backend</p>
              <p className="text-primary">
                {snapshot.recent_ledger.most_recent_effective_backend}
                {snapshot.recent_ledger.most_recent_effective_model
                  ? `/${snapshot.recent_ledger.most_recent_effective_model}`
                  : ''}
              </p>
            </div>
            <div>
              <p className="text-xs text-muted uppercase tracking-wide mb-1">Validation</p>
              <p className="text-primary">{snapshot.recent_ledger.most_recent_validation_result ?? 'Unknown'}</p>
            </div>
            <div>
              <p className="text-xs text-muted uppercase tracking-wide mb-1">When</p>
              <p className="text-primary">
                {formatAge(snapshot.recent_ledger.most_recent_dispatch_timestamp) ?? 'Unknown'}
                {isStale(snapshot.recent_ledger.most_recent_dispatch_timestamp) && (
                  <span className="ml-1 text-muted">(stale)</span>
                )}
              </p>
            </div>
          </div>
          {snapshot.recent_ledger.most_recent_mode === 'review' && (
            <p className="mt-3 text-xs text-secondary">
              Review supervision: {snapshot.recent_ledger.review_timeout_class ?? 'completed'}
              {snapshot.recent_ledger.review_idle_timeout_seconds != null
                ? ` · idle ${snapshot.recent_ledger.review_idle_timeout_seconds}s`
                : ''}
              {snapshot.recent_ledger.review_hard_timeout_seconds != null
                ? ` · hard ${snapshot.recent_ledger.review_hard_timeout_seconds}s`
                : ' · no hard ceiling'}
              {snapshot.recent_ledger.review_last_progress_secs != null
                ? ` · last progress +${Math.round(snapshot.recent_ledger.review_last_progress_secs)}s`
                : ' · no observed progress'}
            </p>
          )}
        </section>
      )}
    </div>
  );
}
