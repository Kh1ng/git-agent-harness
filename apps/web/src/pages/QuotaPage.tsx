import { useEffect, useState } from 'react';
import type { QuotaCheck, QuotaSnapshot, QuotaCandidateStatus, FleetQuotaSnapshot, AccountUsageObservation, AccountUsageModel } from '@git-agent-harness/contracts';
import { Gauge, ListChecks, CheckCircle2, Coins, Timer } from 'lucide-react';
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
import { CliRouterPanel } from '../components/CliRouterPanel.js';
import { gahApi } from '../api/client.js';


const SNAPSHOT_REFRESH_MS = 5 * 60 * 1000;

/** A quota/availability scope's identity string -- backend + instance
 * (model) + pool must never be collapsed, per the spec: "agy" and
 * "agy-second" are different instances, "5-hour" and "weekly" are
 * different windows. This is used as the React key and the card title. */
function scopeIdentity(candidate: Pick<QuotaCandidateStatus, 'backend' | 'provider' | 'backend_instance' | 'quota_pool'> & { model?: string | null }): string {
  const parts = [providerLabel(candidateProvider(candidate))];
  if (candidate.backend_instance) parts.push(candidate.backend_instance);
  if (candidate.quota_pool) parts.push(candidate.quota_pool);
  if (candidate.model) parts.push(candidate.model);
  return [...new Set(parts)].join(' / ');
}

function formatQuotaMetadata(q: {
  quota_window?: string | null;
  quota_reset_at?: string | null;
  usage_source?: string | null;
}): string {
  const remaining = formatRemaining(q.quota_reset_at);
  const reset = q.quota_reset_at?.startsWith('in ')
    ? q.quota_reset_at
    : remaining
      ? `in ${remaining}`
      : formatLocalTime(q.quota_reset_at) ?? q.quota_reset_at;

  return [reset ? `Resets ${reset}` : 'No reset time', q.usage_source ? `source: ${q.usage_source}` : null]
    .filter(Boolean)
    .join(' · ');
}

function quotaPercentages(q: {
  quota_used_percent?: number | null;
  quota_remaining_percent?: number | null;
}): { used: number; remaining: number } | null {
  const used = Number.isFinite(q.quota_used_percent) ? q.quota_used_percent : null;
  const remaining = Number.isFinite(q.quota_remaining_percent) ? q.quota_remaining_percent : null;
  if (used === null || used === undefined) {
    if (remaining === null || remaining === undefined) return null;
    const boundedRemaining = Math.min(100, Math.max(0, remaining));
    return { used: 100 - boundedRemaining, remaining: boundedRemaining };
  }
  const boundedUsed = Math.min(100, Math.max(0, used));
  return {
    used: boundedUsed,
    remaining: remaining === null || remaining === undefined
      ? 100 - boundedUsed
      : Math.min(100, Math.max(0, remaining))
  };
}

function formatQuotaPercent(value: number): string {
  return formatPercent(value / 100, Number.isInteger(value) ? 0 : 1);
}

export function QuotaFreshnessPanel({
  generatedAt,
  freshness,
  quotaChecks
}: {
  generatedAt?: string | null;
  freshness?: QuotaSnapshot['freshness'];
  quotaChecks: QuotaCheck[];
}) {
  return (
    <section className="card-padded">
      <div className="flex flex-wrap items-center justify-between gap-2 mb-3">
        <h3 className="text-sm font-semibold text-primary">Data freshness</h3>
        <span className="text-xs text-muted">
          Snapshot {formatAge(generatedAt) ?? 'not generated'}
        </span>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3 text-xs">
        {([
          ['Ledger activity', freshness?.ledger_observed_at],
          ['Availability check', freshness?.availability_observed_at],
          ['Account quota check', freshness?.quota_checked_at],
          ['Quota data', freshness?.quota_observed_at]
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
      {quotaChecks.length > 0 && (
        <div className="mt-3 pt-3 border-t border-subtle">
          <p className="text-[11px] uppercase tracking-wide text-muted mb-2">
            Account quota checks · shared across profiles
          </p>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {quotaChecks.map((check) => {
              const label = check.status === 'failed'
                ? 'Check failed'
                : check.status === 'no_data'
                  ? 'No quota data recorded'
                  : 'Quota data received';
              const tone = check.status === 'failed'
                ? 'critical'
                : check.status === 'no_data'
                  ? 'unknown'
                  : 'good';
              return (
                <div key={scopeIdentity(check)} data-testid={`quota-check-${check.backend}`} className="text-xs text-secondary">
                  <div className="flex items-center justify-between gap-2">
                    <span>{scopeIdentity(check)}</span>
                    <span className="inline-flex items-center gap-2">
                      {isStale(check.checked_at) && <StatusBadge tone="serious" label="Stale" />}
                      <StatusBadge tone={tone} label={label} />
                    </span>
                  </div>
                  <p className="text-muted mt-1">Checked {formatAge(check.checked_at) ?? check.checked_at}</p>
                  {check.error && <p className="text-critical mt-1">{check.error}</p>}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </section>
  );
}

export function QuotaPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const quota = useGahStore((s) => s.quota);
  const fetchQuota = useGahStore((s) => s.fetchQuota);
  const [fleetRefresh, setFleetRefresh] = useState(0);

  useEffect(() => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' });
  }, [profile, fetchQuota]);

  const refresh = () => {
    fetchQuota({ profile: profile ?? undefined, since: '7d' }, { force: true });
    setFleetRefresh(value => value + 1);
  };
  useAutoRefresh(refresh, SNAPSHOT_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const header = (
    <PageHeader
      title="Quota management"
      description="Account allowances and configured routing candidates"
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
      <div className="quota-page space-y-6">
        {header}
        <CliRouterPanel />
        <LoadingState label="Loading quota snapshot…" />
        <FleetQuotaPanel profile={profile} refreshKey={fleetRefresh} />
      </div>
    );
  }
  if (quota.error && !quota.data) {
    return (
      <div className="quota-page space-y-6">
        {header}
        <CliRouterPanel />
        <ErrorState message={quota.error} endpoint="/api/quota" onRetry={refresh} />
        <FleetQuotaPanel profile={profile} refreshKey={fleetRefresh} />
      </div>
    );
  }

  const snapshot = quota.data;
  const candidates = snapshot?.candidates ?? [];
  const usage = snapshot?.usage;
  const freshness = snapshot?.freshness;

  return (
    <div className="quota-page space-y-6">
      {header}

      <CliRouterPanel />

      <QuotaCandidateLedger candidates={candidates} quotaChecks={snapshot?.quota_checks ?? []} />

      <FleetQuotaPanel profile={profile} refreshKey={fleetRefresh} />

      <details className="quota-settings">
        <summary className="text-sm text-secondary cursor-pointer py-3">Usage and data freshness</summary>
        <div className="space-y-4 pt-2">
          <QuotaFreshnessPanel generatedAt={snapshot?.generated_at} freshness={freshness} quotaChecks={snapshot?.quota_checks ?? []} />
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
              hint={candidates.length > 0 ? `${candidates.filter((c) => c.eligible_now).length} not blocked` : undefined}
            />
          </section>

        </div>
      </details>
    </div>
  );
}

function FleetQuotaPanel({ profile, refreshKey }: { profile: string | null; refreshKey: number }) {
  const [snapshot, setSnapshot] = useState<{ profile: string | null; data: FleetQuotaSnapshot | null; error: string | null } | null>(null);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let cancelled = false;
    gahApi.getFleetQuota({ profile: profile ?? undefined, since: '7d' })
      .then(data => { if (!cancelled) setSnapshot({ profile, data, error: null }); })
      .catch(error => { if (!cancelled) setSnapshot(previous => ({ profile, data: previous?.profile === profile ? previous.data : null, error: error instanceof Error ? error.message : 'Could not load worker quota.' })); });
    return () => { cancelled = true; };
  }, [profile, refreshKey, retry]);
  const current = snapshot?.profile === profile ? snapshot : null;
  return <>
    {!current && <LoadingState label="Loading worker quota…" />}
    {current?.error && <ErrorState message={current.error} endpoint="/api/registry/quota" onRetry={() => setRetry(value => value + 1)} />}
    {current?.data && <FleetQuotaLedger snapshot={current.data} />}
  </>;
}

export function FleetQuotaLedger({ snapshot }: { snapshot: FleetQuotaSnapshot }) {
  if (snapshot.nodes.length === 0) return null;
  return <section className="space-y-6" aria-label="Worker quota observations">
    <div>
      <h3 className="text-base font-semibold text-primary">Worker quota</h3>
      <p className="text-xs text-muted mt-1">Allowances reported on each node. Worker usage ledgers are shown separately and are not added to central totals.</p>
    </div>
    {snapshot.nodes.map(node => <section key={node.nodeId} data-testid={`node-quota-${node.nodeId}`} className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div><h4 className="text-sm font-semibold text-primary">{node.displayName}</h4><p className="text-xs text-muted break-all">Node {node.nodeId}{node.quota ? ` · Snapshot ${formatAge(node.quota.generated_at) ?? node.quota.generated_at}` : ''}</p></div>
        <StatusBadge tone={node.state === 'available' ? 'good' : node.state === 'unavailable' ? 'critical' : 'unknown'} label={node.state === 'available' ? 'Reported' : node.state === 'unavailable' ? 'Unavailable' : 'Project not configured'} />
      </div>
      {node.error && <p className="text-xs text-critical">{node.error}</p>}
      {node.quota && <QuotaCandidateLedger candidates={node.quota.candidates} quotaChecks={node.quota.quota_checks} />}
    </section>)}
  </section>;
}

/** A provider groups routing scopes, without treating distinct models as accounts. */
function candidateProvider(candidate: Pick<QuotaCandidateStatus, 'backend' | 'provider'> & { model?: string | null }): string {
  if (candidate.provider) return candidate.provider;
  // The legacy second AGY runner is another account of the same provider.
  if (candidate.backend === 'agy-second') return 'agy';
  if (candidate.backend === 'mistral-dashboard') return 'mistral';
  return candidate.backend === 'opencode' && candidate.model?.includes('/')
    ? candidate.model.split('/')[0]
    : candidate.backend === 'opencode' ? 'Unknown provider' : candidate.backend;
}

function providerLabel(provider: string): string {
  return ({ nous: 'Nous', 'nous-portal': 'Nous', mistral: 'Mistral', 'mistral-dashboard': 'Mistral', antigravity: 'Antigravity', anthropic: 'Anthropic', openai: 'OpenAI', 'gah-router': 'CLI subscription router' } as Record<string, string>)[provider] ?? provider;
}

type QuotaLedgerRow = Omit<QuotaCandidateStatus, 'usage'> & {
  usage: QuotaCandidateStatus['usage'] | null;
  observationOnly?: boolean;
};

function AccountUsageMetrics({ usage }: { usage: AccountUsageObservation | AccountUsageModel }) {
  const connector = 'usage_type' in usage && usage.usage_type === 'vibe_connectors';
  const metrics: [string, string][] = [
    ['Requests', formatCount(usage.requests)],
    ['Input tokens', formatCount(usage.input_tokens)],
    ['Cached input tokens', formatCount(usage.cached_input_tokens)],
    ['Output tokens', formatCount(usage.output_tokens)],
    ['Consumption cost (USD)', formatCost(usage.cost)]
  ];
  return <dl className="grid grid-cols-2 gap-x-4 gap-y-2 tabular-nums">
    {metrics.filter(([label]) => !connector || !label.includes('tokens')).map(([label, value]) => <div key={label}><dt className="text-muted">{label}</dt><dd className="text-primary">{value}</dd></div>)}
  </dl>;
}

function AccountUsageDetails({ usage }: { usage: AccountUsageObservation }) {
  const period = (value: string) => new Date(value).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' });
  return <div className="space-y-3" data-testid="provider-account-usage">
    <p className="text-muted break-words">{usage.workspace_id === null ? 'Organization scope' : `Workspace ${usage.workspace_id}`} · Account {usage.account_id}</p>
    <p className="text-muted" title={`${usage.period_start} – ${usage.period_end}`}>Period {period(usage.period_start)} – {period(usage.period_end)} (UTC)</p>
    <AccountUsageMetrics usage={usage} />
    <p className="text-muted">{usage.cost_source === 'dashboard_prices' ? 'Consumption priced at dashboard rates.' : 'Consumption cost source unavailable.'}</p>
    <details>
      <summary className="cursor-pointer text-primary">Usage by model ({usage.models.length})</summary>
      <div className="space-y-3 mt-3">{usage.models.map((model, index) => <div key={`${model.model}-${model.usage_type}-${index}`} className="border-t border-subtle pt-3">
        <p className="font-medium text-primary break-words mb-2">{model.model} · {model.usage_type === 'vibe_connectors' ? 'Vibe connector' : 'Vibe'}</p>
        <AccountUsageMetrics usage={model} />
      </div>)}</div>
    </details>
  </div>;
}

function QuotaCandidateLedger({ candidates: configuredCandidates, quotaChecks }: { candidates: QuotaCandidateStatus[]; quotaChecks: QuotaCheck[] }) {
  // An account observation is not permission to schedule work on that account.
  const observedAccounts: QuotaLedgerRow[] = quotaChecks.filter(check =>
    check.backend_instance && ((check.quota_observations?.length ?? 0) > 0 || check.status === 'failed') && !configuredCandidates.some(candidate =>
      candidate.backend === check.backend && candidate.backend_instance === check.backend_instance &&
      (!check.model || candidate.model === check.model) && (!check.quota_pool || candidate.quota_pool === check.quota_pool)
    )
  ).map(check => ({
    backend: check.backend, provider: check.provider, backend_instance: check.backend_instance,
    model: check.model ?? null, quota_pool: check.quota_pool, quota_observations: check.quota_observations,
    modes: [], configured: false, eligible_now: false, observed_at: null, usage: null, observationOnly: true
  }));
  const candidates: QuotaLedgerRow[] = [...configuredCandidates, ...observedAccounts];
  const [providerFilter, setProviderFilter] = useState('All');
  const providers = [...new Set(candidates.map(candidateProvider))];
  const activeFilter = providers.includes(providerFilter) ? providerFilter : 'All';
  const filteredProviders = activeFilter === 'All' ? providers : [activeFilter];
  return (
    <section className="quota-candidates">
      <h3 className="text-base font-semibold text-primary mb-3">Accounts and routing candidates</h3>
      {quotaChecks.filter(check => check.status === 'failed').map((check, index) => (
        <p key={index} role="alert" className="text-xs text-critical mb-3">
          {scopeIdentity(check)} · Quota check failed{check.error ? `: ${check.error}` : '. Refresh the account quota check.'}
        </p>
      ))}
      {candidates.length === 0 ? (
        <EmptyState icon={Gauge} title="No canonical candidates recorded" description="Add routing candidates to the profile to see availability and quota state here." />
      ) : (
        <>
          <div className="quota-provider-tabs" role="group" aria-label="Filter candidates by provider">
            {['All', ...providers].map(provider => (
              <button key={provider} className="quota-provider-tab" aria-pressed={activeFilter === provider} onClick={() => setProviderFilter(provider)}>
                <Gauge size={15} aria-hidden="true" />{providerLabel(provider)}
                <span className="quota-provider-count">({provider === 'All' ? candidates.length : candidates.filter(candidate => candidateProvider(candidate) === provider).length})</span>
              </button>
            ))}
          </div>
          <div className="quota-summary-band" aria-label="Configured provider summaries">
            {filteredProviders.map(provider => {
              const scopes = candidates.filter(candidate => candidateProvider(candidate) === provider);
              const routingScopes = scopes.filter(candidate => !candidate.observationOnly);
              const observations = scopes.flatMap(candidate => candidate.quota_observations ?? []);
              return (
                <section key={provider} className="quota-provider-summary">
                  <div className="flex items-center justify-between gap-3 mb-3"><h4 className="text-sm font-semibold text-primary">{providerLabel(provider)}</h4><span className="text-xs text-muted">{routingScopes.length} {routingScopes.length === 1 ? 'candidate' : 'candidates'}{scopes.length > routingScopes.length ? ` · ${scopes.length - routingScopes.length} observed accounts` : ''}</span></div>
                  {routingScopes.length > 0 ? <p className="text-2xl font-semibold text-primary tabular-nums">{routingScopes.filter(candidate => candidate.eligible_now).length}<span className="text-sm font-normal text-muted"> / {routingScopes.length} not blocked</span></p> : <p className="text-sm text-muted">No routing candidate</p>}
                  <p className="text-xs text-muted mt-2">{observations.length} account {observations.length === 1 ? 'observation' : 'observations'} · {observations.filter(observation => quotaPercentages(observation) !== null).length} with quota percentages</p>
                </section>
              );
            })}
          </div>
          <div className="quota-provider-ledgers">
            {filteredProviders.map(provider => (
              <section key={provider}>
                <h4 className="text-sm font-semibold text-primary mb-2">{providerLabel(provider)}</h4>
                {candidates.filter(candidate => candidateProvider(candidate) === provider).map((candidate, index) => (
                  <div key={index} className="quota-candidate-row" data-testid={`quota-candidate-${candidate.backend}-${index}`}>
                    <div className="min-w-0">
                      <p className="text-sm font-medium text-primary break-words">{scopeIdentity(candidate)}</p>
                      <p className="text-xs text-muted mt-1">{candidate.observationOnly ? 'Observed account · not a routing candidate' : candidate.modes.length > 0 ? candidate.modes.join(', ') : 'candidate'}{candidate.backend === 'opencode' && ' · OpenCode runner'}{!candidate.observationOnly && !candidate.configured && ' · no profile runner override'}</p>
                      <div className="mt-2 flex flex-wrap gap-2">
                        <StatusBadge tone={candidate.observationOnly ? 'unknown' : !candidate.eligible_now ? 'critical' : candidate.observed_at ? 'good' : 'unknown'} label={candidate.observationOnly ? 'Availability unverified' : !candidate.eligible_now ? 'Unavailable' : candidate.observed_at ? 'Eligible' : 'Availability unverified'} />
                        {!(candidate.quota_observations ?? []).some(observation => quotaPercentages(observation) !== null) && <StatusBadge tone="unknown" label="Quota unknown" />}
                        {isStale(candidate.observed_at) && <StatusBadge tone="serious" label="Stale" />}
                      </div>
                      {!candidate.observationOnly && !candidate.eligible_now && <p className="text-xs text-secondary mt-2">{candidate.reason ?? 'Unknown reason'} · {formatRemaining(candidate.unavailable_until) ? `Resets in ${formatRemaining(candidate.unavailable_until)}` : 'No known reset time'}</p>}
                      {!candidate.observationOnly && <p className="text-xs text-muted mt-2">{formatAge(candidate.observed_at) ? `Observed ${formatAge(candidate.observed_at)}` : 'No observation'}</p>}
                    </div>
                    <div className="quota-account-windows">
                      {(candidate.quota_observations ?? []).length === 0 && <p className="text-xs text-muted">No quota windows reported for this candidate.</p>}
                      {(candidate.quota_observations ?? []).map((observation, observationIndex) => {
                        const percentages = quotaPercentages(observation);
                        const window = observation.quota_window === 'vibe-code-included-monthly' ? 'Vibe Code included monthly allowance' : observation.quota_window ?? 'Unknown window';
                        const label = `${window}${observation.model ? ` · ${observation.model}` : ''}`;
                        return (
                          <div key={observationIndex} className="min-w-0 text-xs text-secondary">
                            <div className="flex flex-wrap items-start justify-between gap-2 mb-2"><span>{observation.account_usage ? 'Account consumption' : label}</span>{isStale(observation.observed_at) && <StatusBadge tone="serious" label="Stale" />}</div>
                            {observation.account_usage && <AccountUsageDetails usage={observation.account_usage} />}
                            {percentages ? (
                              <>
                                {observation.account_usage && <p className="text-primary mt-4 mb-2">{label}</p>}
                                <div className="flex items-baseline justify-between gap-2 tabular-nums mb-2"><span>{formatQuotaPercent(percentages.used)} used</span><span className="font-semibold text-primary">{formatQuotaPercent(percentages.remaining)} remaining</span></div>
                                <progress className={`usage-progress router-quota-progress ${percentages.remaining < 20 ? 'text-critical' : percentages.remaining < 60 ? 'text-warning' : 'text-good'}`} max={100} value={percentages.remaining} aria-label={`${label}: ${formatQuotaPercent(percentages.used)} used, ${formatQuotaPercent(percentages.remaining)} remaining`} />
                              </>
                            ) : observation.account_usage
                              ? observation.quota_window === 'vibe-code-included-monthly' && <p className="text-muted mt-4">Monthly allowance reading unavailable</p>
                              : <p className="text-muted">No usage percentage available</p>}
                            <p className="mt-2 text-muted">{observation.account_usage && !percentages ? `source: ${observation.usage_source ?? 'Unknown'}` : formatQuotaMetadata(observation)}</p>
                            <p className="mt-1 text-muted">{formatAge(observation.observed_at) ? `Observed ${formatAge(observation.observed_at)}` : 'No observation time'}</p>
                          </div>
                        );
                      })}
                    </div>
                    {candidate.usage && <details className="quota-row-feedback text-xs text-muted">
                      <summary className="cursor-pointer">Usage details</summary>
                      <div className="flex flex-wrap gap-x-4 gap-y-1 mt-2">
                        <span>{formatCount(candidate.usage.entries)} entries · {formatPercent(candidate.usage.success_rate)} success</span>
                        <span>{formatTokens(candidate.usage.total_tokens)} tokens · {formatCount(candidate.usage.requests_count)} requests</span>
                        {(candidate.usage.actual_cost_usd !== null || candidate.usage.estimated_cost_usd !== null) && <span>Cost: {formatCost(candidate.usage.actual_cost_usd ?? candidate.usage.estimated_cost_usd)}</span>}
                        {candidate.source && <span>Source: {candidate.source}</span>}
                      </div>
                    </details>}
                  </div>
                ))}
              </section>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
