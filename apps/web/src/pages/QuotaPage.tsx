import { useEffect } from 'react';
import { Gauge, Clock, ListChecks, CheckCircle2, Coins, Timer } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { StatTile } from '../components/ui/StatTile.js';
import { formatPercent, formatRemaining, formatAge, isStale, formatTokens, formatCount, formatCost } from '../lib/format.js';

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
  const quota = useGahStore((s) => s.quota);
  const fetchQuota = useGahStore((s) => s.fetchQuota);

  useEffect(() => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' });
  }, [profile, fetchQuota]);

  const refresh = () => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' }, { force: true });
  };

  const header = (
    <PageHeader
      title="Quota"
      description="Canonical candidate usage, availability, and quota observations"
      onRefresh={refresh}
      refreshing={quota.loading}
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

  return (
    <div className="space-y-6">
      {header}

      <section className="grid grid-cols-2 lg:grid-cols-4 gap-3">
        <StatTile
          label="Entries (7d)"
          value={usage?.entries ? formatCount(usage.entries) : 'No data'}
          icon={ListChecks}
          hint={usage?.validation_pass !== null && usage?.validation_pass !== undefined ? `${usage.validation_pass} validated` : undefined}
        />
        <StatTile
          label="Success rate"
          value={formatPercent(usage?.success_rate)}
          icon={CheckCircle2}
          hint={usage?.entries ? `${usage.validation_pass}/${usage.entries} validated` : undefined}
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
                        {!candidate.configured && ' · not configured'}
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
                          <span className="text-right">
                            {q.quota_remaining_percent !== null && q.quota_remaining_percent !== undefined
                              ? `${formatPercent(q.quota_remaining_percent / 100)} remaining`
                              : 'No percentage reported'}
                          </span>
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
