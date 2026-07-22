import { useEffect } from 'react';
import { Gauge, Clock, ListChecks, CheckCircle2, Coins, Timer } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { StatTile } from '../components/ui/StatTile.js';
import { formatPercent, formatRemaining, formatAge, isStale, formatTokens, formatCount, formatCost, formatLocalTime } from '../lib/format.js';

const SNAPSHOT_REFRESH_MS = 5 * 60 * 1000;

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

function formatQuotaUsage(q: {
  quota_used_percent?: number | null;
  quota_remaining_percent?: number | null;
  quota_window?: string | null;
  quota_reset_at?: string | null;
  usage_source?: string | null;
}): string {
  const limit = q.quota_remaining_percent !== null && q.quota_remaining_percent !== undefined
    ? `${formatPercent(q.quota_remaining_percent / 100)} remaining`
    : q.quota_used_percent !== null && q.quota_used_percent !== undefined
      ? `${formatPercent(q.quota_used_percent / 100)} used`
      : 'No usage percentage available';

  const reset = q.quota_reset_at
    ? q.quota_reset_at.startsWith('in ')
      ? q.quota_reset_at
      : formatRemaining(q.quota_reset_at) ?? formatLocalTime(q.quota_reset_at) ?? q.quota_reset_at
    : null;

  const source = q.usage_source ? ` · source: ${q.usage_source}` : '';
  return `${limit}${reset ? ` · resets ${reset}` : ''}${source}`;
}

export function QuotaPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const quota = useGahStore((s) => s.quota);
  const fetchQuota = useGahStore((s) => s.fetchQuota);

  useEffect(() => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' });
  }, [profile, fetchQuota]);

  const refresh = () => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' }, { force: true });
  };
  useAutoRefresh(refresh, SNAPSHOT_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const header = (
    <PageHeader
      title="Quota"
      description="Canonical candidate usage, availability, and quota observations"
      onRefresh={refresh}
      refreshing={quota.loading}
      lastUpdated={quota.fetchedAt}
    />
  );

  // The page's own title/refresh control renders unconditionally -- only
  // the content area swaps to loading/error, so a total data-fetch failure
  // never leaves the user looking at a page with no identity and no way
  // to retry.
  if (quota.loading && !quota.data) {
    return (
      <div className="space-y-6">
        {header}
        <LoadingState label="Loading quota snapshot…" />
      </div>
    );
  }
  if (quota.error && !quota.data) {
    return (
      <div className="space-y-6">
        {header}
        <ErrorState message={quota.error} endpoint="/api/quota" onRetry={refresh} />
      </div>
    );
  }

  const snapshot = quota.data;
  const candidates = snapshot?.candidates ?? [];
  const usage = snapshot?.usage;
  const freshness = snapshot?.freshness;

  return (
    <div className="space-y-6">
      {header}

      <section className="card-padded">
        <div className="flex flex-wrap items-center justify-between gap-2 mb-3">
          <h3 className="text-sm font-semibold text-primary">Data freshness</h3>
          <span className="text-xs text-muted">
            Snapshot {formatAge(snapshot?.generated_at) ?? 'not generated'}
          </span>
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 text-xs">
          {([
            ['Ledger activity', freshness?.ledger_observed_at],
            ['Availability check', freshness?.availability_observed_at],
            ['Quota observation', freshness?.quota_observed_at]
          ] as const).map(([label, observedAt]) => (
            <div key={label} className="flex items-center justify-between gap-2">
              <span className="text-secondary">{label}</span>
              <span className="inline-flex items-center gap-2 text-muted">
                {formatAge(observedAt) ?? 'Never observed'}
                {isStale(observedAt) && <StatusBadge tone="serious" label="Stale" />}
              </span>
            </div>
          ))}
        </div>
      </section>

      <section className="grid grid-cols-2 lg:grid-cols-4 gap-3">
        <StatTile
          label="Entries (7d)"
          value={formatCount(usage?.entries)}
          icon={ListChecks}
          hint={usage?.validation_pass !== null && usage?.validation_pass !== undefined ? `${usage.validation_pass} validated` : undefined}
        />
        <StatTile
          label="Success rate"
          value={formatPercent(usage?.success_rate)}
          icon={CheckCircle2}
          hint={usage?.entries !== null && usage?.entries !== undefined ? `${usage.validation_pass}/${usage.entries} validated` : undefined}
        />
        <StatTile
          label="Usage (7d)"
          value={formatTokens(usage?.total_tokens)}
          icon={Coins}
          hint={usage?.requests_count !== null && usage?.requests_count !== undefined ? `${formatCount(usage.requests_count)} requests` : undefined}
        />
        <StatTile
          label="Candidates"
          value={String(candidates.length)}
          icon={Timer}
          hint={candidates.length > 0 ? `${candidates.filter((c) => c.eligible_now).length} eligible now` : undefined}
        />
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Configured candidates</h3>
        {candidates.length === 0 ? (
          <EmptyState
            icon={Gauge}
            title="No canonical candidates recorded"
            description="Add routing candidates to the profile to see availability and quota state here."
          />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {candidates.map((candidate, i) => {
              const remaining = formatRemaining(candidate.unavailable_until);
              const age = formatAge(candidate.observed_at);
              const stale = isStale(candidate.observed_at);
              const observations = candidate.quota_observations ?? [];
              return (
                <div key={i} className="card-padded">
                  <div className="flex items-start justify-between gap-2 mb-2">
                    <div>
                      <span className="text-sm font-medium text-primary">
                        {scopeIdentity(candidate.backend, candidate.model, candidate.quota_pool)}
                      </span>
                      <p className="text-[11px] text-muted mt-1">
                        {candidate.modes.length > 0 ? candidate.modes.join(', ') : 'candidate'}
                        {!candidate.configured && ' · no profile runner override'}
                      </p>
                    </div>
                    <StatusBadge tone={candidate.eligible_now ? 'good' : 'critical'} label={candidate.eligible_now ? 'Eligible' : 'Unavailable'} />
                  </div>

                  <div className="space-y-1 text-xs text-secondary mb-3">
                    <p>Usage: {formatCount(candidate.usage.entries)} entries, {formatPercent(candidate.usage.success_rate)} success</p>
                    <p className="text-muted">
                      Tokens: {formatTokens(candidate.usage.total_tokens)} · Requests: {formatCount(candidate.usage.requests_count)}
                    </p>
                    {candidate.usage.actual_cost_usd !== null || candidate.usage.estimated_cost_usd !== null ? (
                      <p className="text-muted">Cost: {formatCost(candidate.usage.actual_cost_usd ?? candidate.usage.estimated_cost_usd)}</p>
                    ) : null}
                  </div>

                  {!candidate.eligible_now && (
                    <div className="space-y-1 text-xs text-secondary mb-3">
                      <p>Reason: {candidate.reason ?? 'Unknown'}</p>
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
                  {candidate.source && <p className="text-xs text-muted mt-1">Source: {candidate.source}</p>}

                  {observations.length > 0 && (
                    <div className="mt-3 pt-3 border-t border-subtle space-y-1">
                      <p className="text-[11px] uppercase tracking-wide text-muted">Windows</p>
                      {observations.map((q, j) => (
                        <div key={j} className="flex items-start justify-between gap-2 text-xs text-secondary">
                          <span>{q.quota_window ?? 'unknown'}{q.model ? ` · ${q.model}` : ''}</span>
                          <span className="text-right">{formatQuotaUsage(q)}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
