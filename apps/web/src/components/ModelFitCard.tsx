import { useEffect, useState } from 'react';
import { Award, Info } from 'lucide-react';
import type { RoleMetricsReport, RoleModelMetrics } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { formatTokens } from '../lib/format.js';
import { StatusBadge } from './ui/StatusBadge.js';
import { agentDisplayName } from './LiveAgentsCard.js';

const ROLE_LABEL: Record<string, string> = { fix: 'Fix', improve: 'Implement', review: 'Review', pm: 'Plan', experiment: 'Experiment', routine_review: 'Routine review' };
const CONFIDENCE: Record<RoleModelMetrics['confidence'], { label: string; tone: 'good' | 'warning' | 'critical' | 'unknown' }> = {
  high: { label: 'high confidence', tone: 'good' }, medium: { label: 'medium confidence', tone: 'good' }, low: { label: 'low confidence', tone: 'warning' }, none: { label: 'too few to judge', tone: 'unknown' }
};

const pct = (value: number | null) => (value === null ? '—' : `${Math.round(value * 100)}%`);
const roleLabel = (role: string) => ROLE_LABEL[role] ?? role.replace(/_/g, ' ');
/** "Codex gpt-6-sol", with the account only when it is a named one ("Mixed" means attempts spanned accounts). */
const modelLabel = (cell: { backend: string; model: string | null; backend_instance: string | null }) => {
  const account = cell.backend_instance && cell.backend_instance !== cell.backend && !/^mixed$/i.test(cell.backend_instance) ? ` (${cell.backend_instance})` : '';
  return `${agentDisplayName(cell.backend)}${cell.model ? ` ${cell.model}` : ''}${account}`;
};
function duration(seconds: number | null): string {
  if (seconds === null) return '—';
  if (seconds < 90) return `${Math.round(seconds)}s`;
  const minutes = Math.round(seconds / 60);
  return minutes < 90 ? `${minutes} min` : `${(minutes / 60).toFixed(1)} h`;
}

/** A model's row for one role: the numbers a person compares models by. */
function CellRow({ cell }: { cell: RoleModelMetrics }) {
  const reviewer = cell.role === 'review' || cell.role === 'routine_review';
  const confidence = CONFIDENCE[cell.confidence];
  return (
    <tr className={cell.attempts === 0 ? 'opacity-60' : undefined} data-role={cell.role} data-model={modelLabel(cell)}>
      <td className="whitespace-nowrap py-2 pr-4 text-primary">{modelLabel(cell)}</td>
      <td className="py-2 pr-4 tabular-nums">
        {cell.attempts}
        {cell.harness_errors > 0 && <span className="ml-1 text-muted" title="Harness errors (capacity, admission): the model never ran">+{cell.harness_errors} harness</span>}
      </td>
      <td className="py-2 pr-4 tabular-nums" title={cell.delivered_rate_low === null ? undefined : `At least ${pct(cell.delivered_rate_low)} given ${cell.attempts} attempts (95% Wilson lower bound)`}>
        {cell.attempts === 0 ? '—' : <>{pct(cell.delivered_rate)} <span className="text-muted">({cell.delivered}/{cell.attempts})</span></>}
      </td>
      {reviewer ? (
        <>
          <td className="py-2 pr-4 text-xs">{cell.review_verdicts.map(([verdict, count]) => `${verdict.toLowerCase().replace(/_/g, ' ')} ${count}`).join(' · ') || '—'}</td>
          <td className="py-2 pr-4 tabular-nums" title="Reviews that were right (a NEEDS_FIX followed by a passing fix) against ones later contradicted (an APPROVE whose PR needed a fix)">
            {cell.verdicts_vindicated + cell.verdicts_overturned === 0 ? '—' : `${cell.verdicts_vindicated} right · ${cell.verdicts_overturned} overturned`}
          </td>
        </>
      ) : (
        <>
          <td className="py-2 pr-4 tabular-nums" title="Of attempts where validation ran">{cell.validation_ran === 0 ? '—' : <>{pct(cell.validation_pass_rate)} <span className="text-muted">({cell.validation_passed}/{cell.validation_ran})</span></>}</td>
          <td className="py-2 pr-4 tabular-nums" title="Of this model's pull requests that were reviewed, approved on the first review">{cell.reviewed === 0 ? '—' : <>{pct(cell.first_review_acceptance)} <span className="text-muted">({cell.approved_first_review}/{cell.reviewed})</span></>}</td>
        </>
      )}
      <td className="py-2 pr-4 tabular-nums" title={cell.cost_per_delivered_usd !== null ? 'Dollars recorded by the backend' : 'Tokens, since subscription backends record no dollar cost; "per attempt" when not every attempt was measured'}>
        {cell.cost_per_delivered_usd !== null ? `$${cell.cost_per_delivered_usd.toFixed(2)}`
          : cell.tokens_per_delivered !== null ? formatTokens(Math.round(cell.tokens_per_delivered))
          : cell.tokens_per_attempt !== null ? <>{formatTokens(Math.round(cell.tokens_per_attempt))} <span className="text-muted">per attempt</span></>
          : '—'}
      </td>
      <td className="py-2 pr-4 tabular-nums">{duration(cell.median_duration_seconds)}</td>
      <td className="py-2"><StatusBadge tone={confidence.tone} label={confidence.label} /></td>
    </tr>
  );
}

/**
 * Model fit by role: for each kind of job, how every model that tried it
 * did, from the ledger. Harness errors are counted beside attempts, never
 * against the model. "Best fit" ranks by the delivered rate the data
 * supports at least, so a lucky two-for-two does not beat a steady record.
 */
export function ModelFitCard({ profile, since }: { profile: string | null; since: string }) {
  const [report, setReport] = useState<RoleMetricsReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [epoch, setEpoch] = useState(0);
  useEffect(() => {
    let current = true;
    gahApi.getRoleMetrics(profile ?? undefined, since)
      .then((data) => { if (current) { setReport(data); setError(null); } })
      .catch((err) => { if (current) setError(err instanceof Error ? err.message : String(err)); });
    return () => { current = false; };
  }, [profile, since, epoch]);
  useWsReconnectRefresh(() => setEpoch((value) => value + 1));

  const roles = [...new Set(report?.cells.map((cell) => cell.role) ?? [])].sort((a, b) => ['improve', 'fix', 'review', 'pm'].indexOf(a) - ['improve', 'fix', 'review', 'pm'].indexOf(b));
  return (
    <section className="card-padded" aria-labelledby="model-fit-title">
      <div className="mb-1 flex flex-wrap items-center justify-between gap-3">
        <h3 id="model-fit-title" className="text-sm font-semibold text-primary">Model fit by role ({since})</h3>
        {report && <span className="text-xs tabular-nums text-muted">{report.entries} ledger entries · {report.harness_errors} harness errors · {report.skipped} not attempts</span>}
      </div>
      <p className="mb-3 flex items-start gap-1.5 text-xs text-muted">
        <Info size={13} className="mt-0.5 shrink-0" aria-hidden="true" />
        <span>Delivered: a pull request that was not sent back by its first review. Harness errors (capacity, admission) are listed beside attempts and count against nothing. Cost is in tokens because subscription backends record no dollars.</span>
      </p>
      {error && <p role="alert" className="text-sm text-critical">Cannot compute model metrics: {error}</p>}
      {!error && report === null && <p className="text-sm text-muted">Reading the ledger…</p>}
      {report && report.cells.length === 0 && <p className="text-sm text-muted">No attempts by a model in this window.</p>}

      {report && report.best_fit.some((fit) => fit.ranking.length > 0) && (
        <div className="mb-4 rounded-md border border-subtle p-3" aria-labelledby="best-fit-title">
          <h4 id="best-fit-title" className="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted"><Award size={13} className="text-accent" aria-hidden="true" /> Best fit</h4>
          <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4" aria-label="Best fit by role">
            {report.best_fit.filter((fit) => fit.ranking.length > 0).map((fit) => {
              const [top, ...rest] = fit.ranking;
              const judged = top.confidence !== 'none';
              return (
                <li key={fit.role} className="min-w-0" data-role={fit.role}>
                  <p className="text-xs text-muted">{roleLabel(fit.role)}</p>
                  <p className="truncate text-sm font-semibold text-primary">{judged ? modelLabel(top) : 'Too few attempts to say'}</p>
                  <p className="text-[11px] tabular-nums text-muted">
                    {judged ? `at least ${pct(top.score)} delivered · ${top.attempts} attempts · ${CONFIDENCE[top.confidence].label}` : `${modelLabel(top)} leads on ${top.attempts}; 5 attempts needed`}
                  </p>
                  {rest.length > 0 && judged && <p className="truncate text-[11px] text-muted">then {rest.slice(0, 2).map(modelLabel).join(', ')}</p>}
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {roles.map((role) => {
        const reviewer = role === 'review' || role === 'routine_review';
        return (
          <div key={role} className="mb-4 overflow-x-auto" aria-labelledby={`model-fit-${role}`}>
            <h4 id={`model-fit-${role}`} className="mb-1 text-xs font-semibold uppercase tracking-wide text-secondary">{roleLabel(role)}</h4>
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-subtle text-left text-muted">
                  <th className="py-2 pr-4 font-medium">Model</th>
                  <th className="py-2 pr-4 font-medium">Attempts</th>
                  <th className="py-2 pr-4 font-medium">{reviewer ? 'Verdict given' : 'Delivered'}</th>
                  <th className="py-2 pr-4 font-medium">{reviewer ? 'Verdicts' : 'Validation'}</th>
                  <th className="py-2 pr-4 font-medium">{reviewer ? 'Held up' : 'First review'}</th>
                  <th className="py-2 pr-4 font-medium">{reviewer ? 'Tokens per review' : 'Tokens per delivered'}</th>
                  <th className="py-2 pr-4 font-medium">Median time</th>
                  <th className="py-2 font-medium">Confidence</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-subtle">
                {report!.cells.filter((cell) => cell.role === role).map((cell) => <CellRow key={`${cell.backend}:${cell.model ?? ''}`} cell={cell} />)}
              </tbody>
            </table>
          </div>
        );
      })}
    </section>
  );
}
