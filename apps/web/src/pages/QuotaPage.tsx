import { useEffect } from 'react';
import { Gauge, Clock } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { formatPercent, formatRemaining, formatAge, isStale } from '../lib/format.js';

/** A quota/availability scope's identity string -- backend + instance
 * (model) + pool must never be collapsed, per the spec: "agy" and
 * "agy-second" are different instances, "5-hour" and "weekly" are
 * different windows. This is used as the React key and the card title. */
function scopeIdentity(backend: string, model: string | null, pool?: string | null): string {
  const parts = [backend];
  if (pool) parts.push(pool);
  if (model) parts.push(model);
  return parts.join(' / ');
}

export function QuotaPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const status = useGahStore((s) => s.status);
  const report = useGahStore((s) => s.report);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const fetchReport = useGahStore((s) => s.fetchReport);

  useEffect(() => {
    fetchStatus(profile ?? undefined);
    fetchReport({ profile: profile ?? undefined, since: '7d' });
  }, [profile, fetchStatus, fetchReport]);

  const scopes = status.data?.availability ?? [];
  const quotaObservations = (report.data?.comparisons ?? []).flatMap((c) => c.quota_observations);

  const refresh = () => {
    fetchStatus(profile ?? undefined, { force: true });
    fetchReport({ profile: profile ?? undefined, since: '7d' }, { force: true });
  };
  const header = (
    <PageHeader
      title="Quota"
      description="Eligibility and usage observations, per backend instance and window"
      onRefresh={refresh}
      refreshing={status.loading || report.loading}
    />
  );

  // The page's own title/refresh control renders unconditionally -- only
  // the content area swaps to loading/error, so a total data-fetch failure
  // never leaves the user looking at a page with no identity and no way
  // to retry.
  if (status.loading && !status.data) {
    return (
      <div className="space-y-6">
        {header}
        <LoadingState label="Loading availability…" />
      </div>
    );
  }
  if (status.error && !status.data) {
    return (
      <div className="space-y-6">
        {header}
        <ErrorState message={status.error} endpoint="/api/status" onRetry={refresh} />
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {header}

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Availability</h3>
        {scopes.length === 0 ? (
          <EmptyState icon={Gauge} title="No availability state recorded" description="Everything is eligible by default until a backend reports otherwise." />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {scopes.map((scope, i) => {
              const remaining = formatRemaining(scope.unavailable_until);
              const age = formatAge(scope.observed_at);
              const stale = isStale(scope.observed_at);
              return (
                <div key={i} className="card-padded">
                  <div className="flex items-start justify-between gap-2 mb-2">
                    <span className="text-sm font-medium text-primary">
                      {scopeIdentity(scope.backend, scope.model, scope.quota_pool)}
                    </span>
                    <StatusBadge tone={scope.eligible_now ? 'good' : 'critical'} label={scope.eligible_now ? 'Eligible' : 'Unavailable'} />
                  </div>

                  {!scope.eligible_now && (
                    <div className="space-y-1 text-xs text-secondary mb-2">
                      <p>Reason: {scope.reason ?? 'Unknown'}</p>
                      <p className="inline-flex items-center gap-1">
                        <Clock size={11} aria-hidden="true" />
                        {remaining ? `Resets in ${remaining}` : 'No known reset time'}
                      </p>
                    </div>
                  )}

                  <div className="flex items-center justify-between text-xs text-muted pt-2 border-t border-subtle">
                    <span>{age ? `Observed ${age}` : 'No observation'}</span>
                    {stale && <StatusBadge tone="serious" label="Stale" />}
                  </div>
                  {scope.source && <p className="text-xs text-muted mt-1">Source: {scope.source}</p>}
                </div>
              );
            })}
          </div>
        )}
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Quota observations (7d)</h3>
        {report.loading && !report.data ? (
          <LoadingState label="Loading quota observations…" />
        ) : quotaObservations.length === 0 ? (
          <EmptyState
            icon={Gauge}
            title="No quota observations"
            description="A backend hasn't reported used/remaining percentages in this window yet."
          />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {quotaObservations.map((q, i) => {
              const age = formatAge(q.observed_at);
              const stale = isStale(q.observed_at);
              return (
                <div key={i} className="card-padded">
                  <div className="flex items-start justify-between gap-2 mb-2">
                    <span className="text-sm font-medium text-primary">
                      {scopeIdentity(q.backend, q.model ?? null)}
                    </span>
                    {q.quota_window && <span className="text-xs text-muted">{q.quota_window}</span>}
                  </div>

                  {q.quota_remaining_percent !== null && q.quota_remaining_percent !== undefined ? (
                    <>
                      <div className="w-full h-1.5 rounded-full bg-raised overflow-hidden mb-1.5">
                        <div
                          className="h-full rounded-full bg-accent"
                          style={{ width: `${Math.max(0, Math.min(100, q.quota_remaining_percent))}%` }}
                        />
                      </div>
                      <p className="text-xs text-secondary mb-2">
                        {formatPercent((q.quota_remaining_percent ?? 0) / 100)} remaining
                        {q.quota_used_percent !== null && q.quota_used_percent !== undefined
                          ? ` · ${formatPercent(q.quota_used_percent / 100)} used`
                          : ''}
                      </p>
                    </>
                  ) : (
                    <p className="text-xs text-muted mb-2">
                      {q.backend === 'openhands' ? 'No metering available for this backend' : 'No percentage reported'}
                    </p>
                  )}

                  <div className="flex items-center justify-between text-xs text-muted pt-2 border-t border-subtle">
                    <span>{age ? `Observed ${age}` : 'No observation'}</span>
                    {stale && <StatusBadge tone="serious" label="Stale" />}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
