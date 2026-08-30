import { useEffect, useState, useMemo } from 'react';
import { ArrowUpDown, FlaskConical } from 'lucide-react';
import type { BackendModelComparison, ExportHealth, ReportGroupBy, UsageRollupSummary } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { gahApi } from '../api/client.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge, type StatusTone } from '../components/ui/StatusBadge.js';
import { TrendChart } from '../components/TrendChart.js';
import { formatCost, formatDuration, formatPercent, formatTokens, formatCount, formatAge, oldestFetchedAt } from '../lib/format.js';

const TELEMETRY_REFRESH_MS = 60 * 1000;

/** Issue #230: operator-visible tone/label for each export health state. */
const EXPORT_HEALTH_TONE: Record<ExportHealth['status'], { tone: StatusTone; label: string }> = {
  never_run: { tone: 'unknown', label: 'Never run' },
  healthy: { tone: 'good', label: 'Healthy' },
  stale: { tone: 'warning', label: 'Stale' },
  retrying: { tone: 'serious', label: 'Retrying' },
  failed: { tone: 'critical', label: 'Failed' }
};

function ExportHealthCard({ health }: { health: ExportHealth | undefined }) {
  if (!health) return null;
  const { tone, label } = EXPORT_HEALTH_TONE[health.status] ?? EXPORT_HEALTH_TONE.never_run;
  const lastSuccess = formatAge(health.last_success_at);
  const lastAttempt = formatAge(health.last_attempt_at);
  return (
    <section className="card-padded">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h3 className="text-sm font-semibold text-primary">Telemetry export health</h3>
          <StatusBadge tone={tone} label={label} />
        </div>
        <div className="text-xs text-muted">
          {formatCount(health.record_count)} records exported
          {health.exported_watermark ? ` · through ${health.exported_watermark}` : ''}
        </div>
      </div>
      <div className="mt-2 text-xs text-muted space-y-0.5">
        <div>Last attempt: {lastAttempt ?? 'never'}{lastSuccess ? ` · last success: ${lastSuccess}` : ''}</div>
        {health.retry_pending && <div className="text-warning">Retry pending{health.last_error_class ? ` (${health.last_error_class})` : ''}</div>}
        {health.last_error && <div className="truncate" title={health.last_error}>Last error: {health.last_error}</div>}
      </div>
    </section>
  );
}

type SortKey = keyof Pick<
  BackendModelComparison,
  'entries' | 'success_rate' | 'average_duration_seconds' | 'total_tokens' | 'memory_gateway_capture_l0_recorded' | 'actual_cost_usd' | 'estimated_cost_usd'
>;

/** Actual burn GAH itself observed, aggregated from manager-chat session
 * logs (#940): the dispatch-ledger tables above only cover gah-dispatched
 * work, while nearly all work now happens in manager chat. */
function ChatUsageRollupCard({ profile }: { profile: string | undefined }) {
  const [days, setDays] = useState<7 | 30>(7);
  const [rollup, setRollup] = useState<UsageRollupSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    gahApi.getUsageRollup(profile, days)
      .then((data) => { if (!cancelled) setRollup(data); })
      .catch((err) => { if (!cancelled) setError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [profile, days]);

  const rows = rollup?.rows ?? [];
  const byBackend = useMemo(() => {
    const map = new Map<string, { turns: number; total_tokens: number; estimated_cost_usd: number }>();
    for (const row of rows) {
      const agg = map.get(row.backend) ?? { turns: 0, total_tokens: 0, estimated_cost_usd: 0 };
      agg.turns += row.turns;
      agg.total_tokens += row.total_tokens;
      agg.estimated_cost_usd += row.estimated_cost_usd;
      map.set(row.backend, agg);
    }
    return [...map.entries()].sort((a, b) => b[1].total_tokens - a[1].total_tokens);
  }, [rows]);

  return (
    <section className="card-padded">
      <div className="flex flex-wrap items-center justify-between gap-3 mb-3">
        <div>
          <h3 className="text-sm font-semibold text-primary">Actual usage — manager chat</h3>
          <p className="text-xs text-muted mt-0.5">
            Tokens and turns as reported by each backend during chat, aggregated from GAH's own session logs.
            {rollup && rollup.unattributed_turns > 0 && ` ${rollup.unattributed_turns} turn(s) reported no usage.`}
          </p>
        </div>
        <div className="flex rounded-md border border-subtle overflow-hidden text-xs">
          {([7, 30] as const).map((d) => (
            <button
              key={d}
              onClick={() => setDays(d)}
              className={`px-3 py-1.5 ${days === d ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'}`}
            >
              {d}d
            </button>
          ))}
        </div>
      </div>
      {loading && !rollup ? (
        <LoadingState label="Rolling up session usage…" />
      ) : error ? (
        <ErrorState message={`Usage rollup failed: ${error}`} onRetry={() => setDays((current) => current)} />
      ) : rows.length === 0 ? (
        <EmptyState icon={FlaskConical} title="No usage recorded" description={`No manager-chat usage in the last ${days} days.`} />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left text-muted border-b border-subtle">
                <th className="py-2 pr-4 font-medium">Backend</th>
                <th className="py-2 pr-4 font-medium">Turns</th>
                <th className="py-2 pr-4 font-medium">Tokens</th>
                <th className="py-2 pr-4 font-medium">Est. cost</th>
                <th className="py-2 font-medium">By model</th>
              </tr>
            </thead>
            <tbody>
              {byBackend.map(([backend, agg]) => {
                const modelRows = rows.filter((row) => row.backend === backend).sort((a, b) => b.total_tokens - a.total_tokens);
                return (
                  <tr key={backend} className="border-b border-subtle/50">
                    <td className="py-2 pr-4 text-primary">{backend}</td>
                    <td className="py-2 pr-4">{formatCount(agg.turns)}</td>
                    <td className="py-2 pr-4">{formatTokens(agg.total_tokens)}</td>
                    <td className="py-2 pr-4">{agg.estimated_cost_usd > 0 ? formatCost(agg.estimated_cost_usd) : <span className="text-muted">plan</span>}</td>
                    <td className="py-2 text-muted">
                      {modelRows.map((row) => `${row.model ?? 'default'}: ${formatTokens(row.total_tokens)}`).join(' · ')}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/** The most recently observed quota_used_percent for a comparison row, or
 * null if the backend/model has never reported one. A row can carry
 * multiple quota_observations (different windows, e.g. "5-hour" vs
 * "weekly") -- most recent by observed_at wins, matching how QuotaPage
 * already resolves the same ambiguity per scope. Subscription backends
 * (agy, codex/claude CLI) have no real per-token $ cost, so this is the
 * metric that actually means something for them -- cost columns stay for
 * backends that do have one (e.g. metered API usage). */
function latestQuotaUsedPercent(row: BackendModelComparison): { percent: number; window: string | null } | null {
  const withPercent = (row.quota_observations ?? []).filter((q) => q.quota_used_percent !== null && q.quota_used_percent !== undefined);
  if (withPercent.length === 0) return null;
  const latest = withPercent.reduce((a, b) => ((b.observed_at ?? '') > (a.observed_at ?? '') ? b : a));
  return { percent: latest.quota_used_percent as number, window: latest.quota_window ?? null };
}

function SortHeader({ label, active, onClick }: { label: string; active: boolean; onClick: () => void }) {
  return (
    <th>
      <button onClick={onClick} className={`inline-flex items-center gap-1 hover:text-primary ${active ? 'text-primary' : ''}`}>
        {label}
        <ArrowUpDown size={11} aria-hidden="true" />
      </button>
    </th>
  );
}

function UsageSummary({ rows }: { rows: BackendModelComparison[] }) {
  const totalCost = rows.reduce((sum, r) => sum + (r.actual_cost_usd ?? r.estimated_cost_usd ?? 0), 0);
  const totalTokens = rows.reduce((sum, r) => sum + (r.total_tokens ?? 0), 0);
  const maxCost = Math.max(...rows.map((r) => r.actual_cost_usd ?? r.estimated_cost_usd ?? 0), 0.0001);

  return (
    <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
      <div className="card-padded md:col-span-1 space-y-1">
        <p className="text-xs text-muted uppercase tracking-wide">Total spend (7d)</p>
        <p className="text-2xl font-bold text-primary">{formatCost(totalCost)}</p>
        <p className="text-xs text-muted">{formatTokens(totalTokens)} tokens</p>
      </div>
      <div className="card-padded md:col-span-2">
        <p className="text-xs text-muted uppercase tracking-wide mb-3">By provider</p>
        <div className="space-y-2">
          {rows.map((r) => {
            const cost = r.actual_cost_usd ?? r.estimated_cost_usd ?? 0;
            const pct = maxCost > 0 ? (cost / maxCost) * 100 : 0;
            return (
              <div key={r.backend_or_model} className="flex items-center gap-3">
                <span className="text-xs text-secondary w-28 truncate shrink-0">{r.backend_or_model}</span>
                <div className="flex-1 h-1.5 bg-raised rounded-full overflow-hidden">
                  <div className="h-full bg-accent rounded-full" style={{ width: `${pct}%` }} />
                </div>
                <span className="text-xs text-primary font-mono w-16 text-right shrink-0">{formatCost(cost)}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export function TelemetryPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const report = useGahStore((s) => s.report);
  const fetchReport = useGahStore((s) => s.fetchReport);
  const reportSeries = useGahStore((s) => s.reportSeries);
  const fetchReportSeries = useGahStore((s) => s.fetchReportSeries);
  const status = useGahStore((s) => s.status);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const trend = reportSeries.data?.series ?? [];
  const trendOptions = [
    { id: 'tokens', label: 'Input+output tokens', data: trend.map((p) => ({ date: p.date, value: p.total_tokens })), format: formatTokens },
    { id: 'cost', label: 'Cost (USD)', data: trend.map((p) => ({ date: p.date, value: p.actual_cost_usd ?? p.estimated_cost_usd ?? 0 })), format: (v: number) => formatCost(v) },
    { id: 'success', label: 'Success rate', data: trend.map((p) => ({ date: p.date, value: p.success_rate })), format: (v: number) => formatPercent(v) }
  ] as const;
  const [groupBy, setGroupBy] = useState<ReportGroupBy>('backend');
  const [sortKey, setSortKey] = useState<SortKey>('entries');
  const [sortDesc, setSortDesc] = useState(true);
  const [trendMetric, setTrendMetric] = useState<(typeof trendOptions)[number]['id']>('tokens');

  useEffect(() => {
    fetchReport({ profile: profile ?? undefined, since: '7d', groupBy }, { force: true });
    fetchReportSeries({ profile: profile ?? undefined, since: '14d', bucket: 'daily' }, { force: true });
    fetchStatus(profile ?? undefined, { force: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupBy, profile]);

  const refreshAll = () => {
    fetchReport({ profile: profile ?? undefined, since: '7d', groupBy }, { force: true });
    fetchReportSeries({ profile: profile ?? undefined, since: '14d', bucket: 'daily' }, { force: true });
    fetchStatus(profile ?? undefined, { force: true });
  };
  useAutoRefresh(refreshAll, TELEMETRY_REFRESH_MS);
  useWsReconnectRefresh(refreshAll);
  const lastUpdated = oldestFetchedAt(report.fetchedAt, reportSeries.fetchedAt);

  const sorted = useMemo(() => {
    const rows = [...(report.data?.comparisons ?? [])];
    rows.sort((a, b) => {
      const av = a[sortKey];
      const bv = b[sortKey];
      if (av === null && bv === null) return 0;
      if (av === null) return 1; // unknowns sort last, never treated as 0
      if (bv === null) return -1;
      return sortDesc ? (bv as number) - (av as number) : (av as number) - (bv as number);
    });
    return rows;
  }, [report.data, sortKey, sortDesc]);

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDesc((d) => !d);
    } else {
      setSortKey(key);
      setSortDesc(true);
    }
  };

  const activeTrend = trendOptions.find((t) => t.id === trendMetric)!;

  const cacheHitRatio = (row: BackendModelComparison): number | null => {
    if (row.cache_read_tokens === null || row.cache_write_tokens === null) return null;
    const cacheBase = row.cache_read_tokens + row.cache_write_tokens;
    return cacheBase > 0 ? row.cache_read_tokens / cacheBase : 0;
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Telemetry"
        description="Backend & model performance, tokens, and cost"
        onRefresh={refreshAll}
        refreshing={report.loading}
        lastUpdated={lastUpdated}
        actions={
          <div className="flex rounded-md border border-subtle overflow-hidden text-xs">
            {(['backend', 'model'] as const).map((g) => (
              <button
                key={g}
                onClick={() => setGroupBy(g)}
                className={`px-3 py-1.5 capitalize ${groupBy === g ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'}`}
              >
                {g}
              </button>
            ))}
          </div>
        }
      />

      <ExportHealthCard health={status.data?.export_health} />

      {sorted.length > 0 && <UsageSummary rows={sorted} />}

      <ChatUsageRollupCard profile={profile ?? undefined} />

      <section className="card-padded">
        <div className="flex flex-wrap items-center justify-between gap-3 mb-3">
          <h3 className="text-sm font-semibold text-primary">Usage trend</h3>
          <div className="flex items-center gap-3">
            <span className="inline-flex items-center gap-1 text-xs text-muted">
              <FlaskConical size={12} aria-hidden="true" />
              Real ledger series ({reportSeries.data?.bucket ?? 'daily'})
            </span>
            <select
              value={trendMetric}
              onChange={(e) => setTrendMetric(e.target.value as typeof trendMetric)}
              className="bg-raised border border-subtle rounded-md px-2 py-1 text-xs text-primary"
            >
              {trendOptions.map((opt) => (
                <option key={opt.id} value={opt.id}>
                  {opt.label}
                </option>
              ))}
            </select>
          </div>
        </div>
        {reportSeries.loading && !reportSeries.data ? (
          <LoadingState label="Loading usage trend…" />
        ) : reportSeries.error ? (
          <ErrorState
            message={reportSeries.error}
            endpoint="/api/report/series"
            onRetry={() => fetchReportSeries({ profile: profile ?? undefined, since: '14d', bucket: 'daily' }, { force: true })}
          />
        ) : activeTrend.data.length === 0 ? (
          <EmptyState icon={FlaskConical} title="No trend data for this window" description="Try a longer time range once more runs have completed." />
        ) : (
          <TrendChart data={activeTrend.data} valueLabel={activeTrend.label} formatValue={activeTrend.format as (v: number) => string} />
        )}
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">
          {groupBy === 'model' ? 'Model' : 'Backend'} performance (7d)
        </h3>
        {report.loading && !report.data ? (
          <LoadingState label="Loading report…" />
        ) : report.error ? (
          <ErrorState
            message={report.error}
            endpoint="/api/report"
            onRetry={() => fetchReport({ profile: profile ?? undefined, since: '7d', groupBy }, { force: true })}
          />
        ) : sorted.length === 0 ? (
          <EmptyState icon={FlaskConical} title="No report data for this window" description="Try a longer time range once more runs have completed." />
        ) : (
          <div className="card overflow-x-auto">
            <table className="table-base min-w-[980px]">
              <thead>
                <tr>
                  <th>{groupBy === 'model' ? 'Model' : 'Backend'}</th>
                  <SortHeader label="Tasks" active={sortKey === 'entries'} onClick={() => toggleSort('entries')} />
                  <SortHeader label="Success rate" active={sortKey === 'success_rate'} onClick={() => toggleSort('success_rate')} />
                  <SortHeader label="Avg duration" active={sortKey === 'average_duration_seconds'} onClick={() => toggleSort('average_duration_seconds')} />
                  <SortHeader label="Total tokens" active={sortKey === 'total_tokens'} onClick={() => toggleSort('total_tokens')} />
                  <SortHeader label="Memory records captured" active={sortKey === 'memory_gateway_capture_l0_recorded'} onClick={() => toggleSort('memory_gateway_capture_l0_recorded')} />
                  <th>Cache read tokens</th>
                  <th>Cache write tokens</th>
                  <th>Cache-hit ratio</th>
                  <SortHeader label="Actual cost" active={sortKey === 'actual_cost_usd'} onClick={() => toggleSort('actual_cost_usd')} />
                  <SortHeader label="Est. cost" active={sortKey === 'estimated_cost_usd'} onClick={() => toggleSort('estimated_cost_usd')} />
                  <th>Cost / success</th>
                  <th>Quota used</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((row) => {
                  const costPerSuccess =
                    row.validation_pass > 0 && (row.actual_cost_usd !== null || row.estimated_cost_usd !== null)
                      ? (row.actual_cost_usd ?? row.estimated_cost_usd ?? 0) / row.validation_pass
                      : null;
                  const quota = latestQuotaUsedPercent(row);
                  return (
                    <tr key={row.backend_or_model}>
                      <td className="text-primary font-medium">{row.backend_or_model}</td>
                      <td>
                        {formatCount(row.entries)}
                        <span className="text-muted"> ({formatCount(row.attempts)} attempts)</span>
                      </td>
                      <td>{formatPercent(row.success_rate)}</td>
                      <td>{formatDuration(row.average_duration_seconds)}</td>
                      <td>{formatTokens(row.total_tokens)}</td>
                      <td>{formatCount(row.memory_gateway_capture_l0_recorded)}</td>
                      <td>{formatTokens(row.cache_read_tokens)}</td>
                      <td>{formatTokens(row.cache_write_tokens)}</td>
                      <td>{formatPercent(cacheHitRatio(row))}</td>
                      <td>{formatCost(row.actual_cost_usd)}</td>
                      <td>{formatCost(row.estimated_cost_usd)}</td>
                      <td>{costPerSuccess !== null ? formatCost(costPerSuccess) : 'Unknown'}</td>
                      <td>
                        {quota ? (
                          <>
                            {formatPercent(quota.percent / 100)}
                            {quota.window && <span className="text-muted"> ({quota.window})</span>}
                          </>
                        ) : (
                          <span className="text-muted">Unknown</span>
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
    </div>
  );
}
