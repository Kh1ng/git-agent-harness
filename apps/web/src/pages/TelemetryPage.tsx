import { useEffect, useState, useMemo } from 'react';
import { ArrowUpDown, FlaskConical } from 'lucide-react';
import type { BackendModelComparison, ReportGroupBy } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { TrendChart } from '../components/TrendChart.js';
import { formatCost, formatDuration, formatPercent, formatTokens, formatCount } from '../lib/format.js';

type SortKey = keyof Pick<
  BackendModelComparison,
  'entries' | 'success_rate' | 'average_duration_seconds' | 'total_tokens' | 'actual_cost_usd' | 'estimated_cost_usd'
>;

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

export function TelemetryPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const report = useGahStore((s) => s.report);
  const fetchReport = useGahStore((s) => s.fetchReport);
  const trend = report.data?.trend ?? [];
  const trendOptions = [
    { id: 'tokens', label: 'Input+output tokens', data: trend.map((p) => ({ date: p.date, value: p.total_tokens })), format: formatTokens },
    { id: 'cost', label: 'Cost (USD)', data: trend.map((p) => ({ date: p.date, value: p.actual_cost_usd ?? p.estimated_cost_usd ?? 0 })), format: (v: number) => formatCost(v) },
    { id: 'success', label: 'Success rate', data: trend.map((p) => ({ date: p.date, value: p.entries ? p.validation_pass / p.entries : 0 })), format: (v: number) => formatPercent(v) }
  ] as const;
  const [groupBy, setGroupBy] = useState<ReportGroupBy>('backend');
  const [sortKey, setSortKey] = useState<SortKey>('entries');
  const [sortDesc, setSortDesc] = useState(true);
  const [trendMetric, setTrendMetric] = useState<(typeof trendOptions)[number]['id']>('tokens');

  useEffect(() => {
    fetchReport({ profile: profile ?? undefined, since: '7d', groupBy }, { force: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupBy, profile]);

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

  return (
    <div className="space-y-6">
      <PageHeader
        title="Telemetry"
        description="Backend & model performance, tokens, and cost"
        onRefresh={() => fetchReport({ profile: profile ?? undefined, since: '7d', groupBy }, { force: true })}
        refreshing={report.loading}
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

      <section className="card-padded">
        <div className="flex flex-wrap items-center justify-between gap-3 mb-3">
          <h3 className="text-sm font-semibold text-primary">Usage trend</h3>
          <div className="flex items-center gap-3">
            <span className="inline-flex items-center gap-1 text-xs text-warning">
              <FlaskConical size={12} aria-hidden="true" />
              Ledger-derived daily data
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
        <TrendChart data={activeTrend.data} valueLabel={activeTrend.label} formatValue={activeTrend.format as (v: number) => string} />
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
            <table className="table-base min-w-[860px]">
              <thead>
                <tr>
                  <th>{groupBy === 'model' ? 'Model' : 'Backend'}</th>
                  <SortHeader label="Tasks" active={sortKey === 'entries'} onClick={() => toggleSort('entries')} />
                  <SortHeader label="Success rate" active={sortKey === 'success_rate'} onClick={() => toggleSort('success_rate')} />
                  <SortHeader label="Avg duration" active={sortKey === 'average_duration_seconds'} onClick={() => toggleSort('average_duration_seconds')} />
                  <SortHeader label="Total tokens" active={sortKey === 'total_tokens'} onClick={() => toggleSort('total_tokens')} />
                  <SortHeader label="Actual cost" active={sortKey === 'actual_cost_usd'} onClick={() => toggleSort('actual_cost_usd')} />
                  <SortHeader label="Est. cost" active={sortKey === 'estimated_cost_usd'} onClick={() => toggleSort('estimated_cost_usd')} />
                  <th>Cost / success</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((row) => {
                  const costPerSuccess =
                    row.validation_pass > 0 && (row.actual_cost_usd !== null || row.estimated_cost_usd !== null)
                      ? (row.actual_cost_usd ?? row.estimated_cost_usd ?? 0) / row.validation_pass
                      : null;
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
                      <td>{formatCost(row.actual_cost_usd)}</td>
                      <td>{formatCost(row.estimated_cost_usd)}</td>
                      <td>{costPerSuccess !== null ? formatCost(costPerSuccess) : 'Unknown'}</td>
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
