import { useEffect } from 'react';
import {
  ListChecks,
  CheckCircle2,
  Coins,
  Timer,
  GitMerge,
  AlertTriangle,
  ShieldAlert,
  Play,
  Square
} from 'lucide-react';
import type { Page } from '../App.js';
import type { Session } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { StatTile } from '../components/ui/StatTile.js';
import { StatusBadge, classificationTone } from '../components/ui/StatusBadge.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { SessionCard } from '../components/SessionCard.js';
import { formatPercent, formatAge, formatLocalTime, isStale, formatTokens, formatCount } from '../lib/format.js';
import { ControllerActivityCard } from '../components/ControllerActivityCard.js';

type OverviewPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
  onNavigate: (page: Page) => void;
};

export function OverviewPage({ sessions, onSelectSession, onNavigate }: OverviewPageProps) {
  const { status, quota, loopStatus, loopAction } = useGahStore();
  const { profile: wsProfile, controllerActivity } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
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

  const refresh = () => {
    fetchStatus(profile ?? undefined, { force: true });
    fetchQuota({ profile: profile ?? undefined, since: '7d' }, { force: true });
  };

  // The page's own title/refresh control renders unconditionally below --
  // only the content area swaps to loading/error, so a total data-fetch
  // failure never leaves the user looking at a page with no identity and
  // no way to retry.
  if ((status.loading && !status.data) || (quota.loading && !quota.data)) {
    return (
      <div className="space-y-6">
        <PageHeader title="Overview" onRefresh={refresh} refreshing />
        <LoadingState label="Loading status…" />
      </div>
    );
  }
  if ((status.error && !status.data) || (quota.error && !quota.data)) {
    return (
      <div className="space-y-6">
        <PageHeader title="Overview" onRefresh={refresh} />
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

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
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
      </div>

      {(blockers.length > 0 || blockedWorkItems.length > 0) && (
        <div className="card-padded border-warning/30">
          <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
            <ShieldAlert size={16} className="text-warning" aria-hidden="true" />
            Needs attention
          </h3>
          <ul className="space-y-2">
            {blockers.map((b, i) => (
              <li key={`blocker-${i}`} className="flex items-start gap-2 text-sm">
                <StatusBadge tone="critical" label={b.kind.replace(/_/g, ' ')} />
                <span className="text-secondary">{b.message || b.reason || 'Unknown'} — blocks all work</span>
              </li>
            ))}
            {blockedWorkItems.map((b, i) => (
              <li key={`work-${i}`} className="flex items-start gap-2 text-sm">
                <StatusBadge tone="warning" label="Human required" />
                <span className="text-secondary">
                  {b.source_reference ?? 'Unknown work item'}
                  {b.message ? ` — ${b.message}` : ''}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <ControllerActivityCard activity={controllerActivity} />
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <section>
          <h3 className="text-sm font-semibold text-primary mb-3">What's running now</h3>
          {activeWorkCount === 0 ? (
            <EmptyState icon={Timer} title="No active sessions" description="Dispatched work will appear here while it runs." />
          ) : (
            <div className="space-y-3">
              {activeControllerRuns.map((run) => (
                <div key={run.run_id} className="card-padded flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="font-mono text-xs text-primary">{run.work_id ?? 'unassigned'}</p>
                    <p className="text-xs text-secondary mt-1">{run.action}</p>
                    <p className="text-[10px] text-muted mt-1 truncate">run {run.run_id}</p>
                  </div>
                  <StatusBadge tone="good" label="running" />
                </div>
              ))}
              {activeSessions.slice(0, 4).map((session) => (
                <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
              ))}
            </div>
          )}
          <button onClick={() => onNavigate('work')} className="text-xs text-accent hover:underline mt-2">
            View all work →
          </button>
        </section>

        <section>
          <h3 className="text-sm font-semibold text-primary mb-3">Backend availability</h3>
          {unavailableBackends.length === 0 && (quotaSnapshot?.candidates.length ?? 0) === 0 ? (
            <EmptyState icon={CheckCircle2} title="No quota snapshot recorded" description="Everything is eligible by default until a configured candidate reports otherwise." />
          ) : unavailableBackends.length === 0 ? (
            <div className="card-padded flex items-center gap-2 text-sm text-good">
              <CheckCircle2 size={16} aria-hidden="true" />
              All configured candidates eligible
            </div>
          ) : (
            <div className="space-y-2">
              {unavailableBackends.map((a, i) => (
                <div key={i} className="card-padded flex items-center justify-between text-sm">
                  <span className="text-primary">
                    {a.model ? `${a.backend}/${a.model}` : a.backend}
                    {a.quota_pool ? ` · ${a.quota_pool}` : ''}
                  </span>
                  <StatusBadge tone="critical" label={a.reason ?? 'unavailable'} />
                </div>
              ))}
            </div>
          )}
          <button onClick={() => onNavigate('quota')} className="text-xs text-accent hover:underline mt-2">
            View quota detail →
          </button>
        </section>
      </div>

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
                      <tr key={mr.branch}>
                        <td className="font-mono text-xs">{mr.branch}</td>
                        <td>
                          <StatusBadge tone={tone} label={label} />
                        </td>
                        <td>
                          {mr.url && (
                            <a href={mr.url} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline text-xs">
                              View MR
                            </a>
                          )}
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
                    <tr key={mr.branch}>
                      <td className="font-mono text-xs whitespace-nowrap">
                        {mr.work_id ?? <span className="text-muted">—</span>}
                      </td>
                      <td className="text-xs max-w-[16rem] truncate" title={mr.title ?? mr.branch}>
                        {mr.url ? (
                          <a href={mr.url} target="_blank" rel="noopener noreferrer" className="text-primary hover:text-accent hover:underline">
                            {mr.title ?? mr.branch}
                          </a>
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
                        {mr.review_verdict ? (
                          <div className="space-y-1">
                            <StatusBadge
                              tone={mr.review_verdict.toLowerCase().includes('approve') ? 'good' : 'warning'}
                              label={mr.review_verdict}
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
                        {mr.url && (
                          <a href={mr.url} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline text-xs whitespace-nowrap">
                            View
                          </a>
                        )}
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
        </section>
      )}
    </div>
  );
}
