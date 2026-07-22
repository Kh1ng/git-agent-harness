import { GitBranch, GitPullRequest, Eye, RotateCw, ShieldAlert, CircleDot } from 'lucide-react';
import type {
  AttemptRecord,
  AttemptRoutingRecord,
  LedgerEntry,
  LedgerUsage,
  RoutingCandidateDiagnostic,
  RoutingDiagnostics
} from '@git-agent-harness/contracts';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { formatCount, formatCost, formatDuration, formatTokens } from '../lib/format.js';

const DISPATCH_REASON_LABEL: Record<string, string> = {
  initial: 'Dispatch',
  post_review_repair: 'Repair',
  review: 'Review',
  stuck_loop_gate: 'Stopped (stuck-loop guard)'
};

const DISPATCH_REASON_ICON: Record<string, typeof GitBranch> = {
  initial: GitBranch,
  post_review_repair: RotateCw,
  review: Eye,
  stuck_loop_gate: ShieldAlert
};

function validationTone(result: string | null | undefined): StatusTone {
  if (!result) return 'unknown';
  if (result === 'passed') return 'good';
  if (result === 'failed-draft') return 'serious';
  return 'critical';
}

function reviewTone(verdict: string | null | undefined): StatusTone {
  switch (verdict) {
    case 'APPROVE_STRONG':
    case 'APPROVE_WEAK':
      return 'good';
    case 'NEEDS_FIX':
      return 'warning';
    case 'REJECT':
      return 'critical';
    case 'HUMAN_REVIEW':
      return 'serious';
    default:
      return 'unknown';
  }
}

function attributionLabel(usage: LedgerUsage, effectiveModel: string | null): string {
  const provider = usage.provider ?? 'Unknown provider';
  const model = usage.actual_model ?? effectiveModel ?? 'Unknown model';
  return `${provider}/${model}`;
}

function attributionUnknownReason(usage: LedgerUsage): string | null {
  return usage.actual_model_unknown_reason ?? usage.provider_unknown_reason ?? null;
}

function usageUnknownReasons(usage: LedgerUsage): string[] {
  return [
    usage.token_usage_unknown_reason ? `Tokens: ${usage.token_usage_unknown_reason}` : null,
    usage.quota_unknown_reason ? `Quota: ${usage.quota_unknown_reason}` : null,
    usage.cost_unknown_reason ? `Cost: ${usage.cost_unknown_reason}` : null
  ].filter((reason): reason is string => reason !== null);
}

function tokenBreakdown(usage: LedgerUsage): string | null {
  const parts = [
    usage.input_tokens != null ? `in ${formatTokens(usage.input_tokens)}` : null,
    usage.output_tokens != null ? `out ${formatTokens(usage.output_tokens)}` : null,
    usage.reasoning_tokens != null ? `reasoning ${formatTokens(usage.reasoning_tokens)}` : null,
    usage.cache_read_tokens != null ? `cache read ${formatTokens(usage.cache_read_tokens)}` : null,
    usage.cache_write_tokens != null ? `cache write ${formatTokens(usage.cache_write_tokens)}` : null
  ].filter((part): part is string => part !== null);
  return parts.length > 0 ? parts.join(' · ') : null;
}

function backendModelLabel(backend: string | null | undefined, model: string | null | undefined): string {
  const backendLabel = backend ?? 'Unknown backend';
  return model ? `${backendLabel}/${model}` : backendLabel;
}

function routingCandidateLabel(candidate: RoutingCandidateDiagnostic): string {
  const parts = [backendModelLabel(candidate.backend, candidate.model)];

  if (candidate.quota_pool) parts.push(`quota ${candidate.quota_pool}`);
  if (candidate.pace_band) parts.push(`pace ${candidate.pace_band}`);
  if (candidate.cost_class) parts.push(`cost ${candidate.cost_class}`);

  return parts.join(' · ');
}

function routingSummary(diagnostics: RoutingDiagnostics): string {
  const parts = [
    `Selected ${backendModelLabel(diagnostics.selected_backend, diagnostics.selected_model)}`,
    diagnostics.selected_quota_pool ? `quota ${diagnostics.selected_quota_pool}` : null,
    diagnostics.selected_pace_band ? `pace ${diagnostics.selected_pace_band}` : null,
    diagnostics.selected_cost_class ? `cost ${diagnostics.selected_cost_class}` : null
  ].filter((part): part is string => part !== null);

  return parts.join(' · ');
}

function diffLabel(value: number | null, label: string): string {
  return `${label}: ${value === null ? 'Unknown' : formatCount(value)}`;
}

function renderRoutingDiagnostics(
  diagnostics: RoutingDiagnostics | null | undefined,
  title: string,
  summary: string | null | undefined,
  confidenceImpact: string | null | undefined
) {
  if (!summary && !confidenceImpact && !diagnostics) return null;

  return (
    <div className="mt-2 space-y-1 text-xs text-muted">
      <p className="text-secondary font-medium">{title}</p>
      {summary && <p>Reason: {summary}</p>}
      {confidenceImpact && <p>Confidence impact: {confidenceImpact}</p>}
      {diagnostics && (
        <>
          <p>Selected candidate: {routingSummary(diagnostics)}</p>
          {diagnostics.human_summary && <p>Routing summary: {diagnostics.human_summary}</p>}
          {diagnostics.policy_reordered_candidates && <p>Policy reordered candidates before selection.</p>}
          {diagnostics.selected_over.length > 0 && (
            <p>Selected over: {diagnostics.selected_over.join(' · ')}</p>
          )}
          <div className="space-y-1">
            {diagnostics.candidates.map((candidate: RoutingCandidateDiagnostic, index: number) => {
              const isSelected =
                candidate.backend === diagnostics.selected_backend &&
                candidate.model === diagnostics.selected_model &&
                candidate.quota_pool === diagnostics.selected_quota_pool;

              return (
                <p key={`${candidate.backend}-${candidate.model ?? 'unknown'}-${candidate.consideration_order ?? index}`}>
                  {routingCandidateLabel(candidate)} ·{' '}
                  {isSelected ? 'selected' : candidate.skip_reason ? `skipped: ${candidate.skip_reason}` : 'considered'}
                  {candidate.unavailable_until ? ` · unavailable until ${candidate.unavailable_until}` : ''}
                </p>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}

function commitPushMrBadges(entry: LedgerEntry) {
  return (
    <div className="flex flex-wrap gap-2">
      <StatusBadge
        tone={outcomeTone(entry.commit_attempted, entry.commit_created)}
        label={outcomeLabel('Commit', entry.commit_attempted, entry.commit_created, 'created')}
      />
      <StatusBadge
        tone={outcomeTone(entry.push_attempted, entry.push_succeeded)}
        label={outcomeLabel('Push', entry.push_attempted, entry.push_succeeded, 'succeeded')}
      />
      <StatusBadge
        tone={outcomeTone(entry.mr_attempted, entry.mr_created)}
        label={outcomeLabel('MR', entry.mr_attempted, entry.mr_created, 'created')}
      />
    </div>
  );
}

function outcomeTone(attempted: boolean, succeeded: boolean): StatusTone {
  if (!attempted) return 'unknown';
  return succeeded ? 'good' : 'warning';
}

function outcomeLabel(label: string, attempted: boolean, succeeded: boolean, successWord: string): string {
  if (!attempted) return `${label}: not attempted`;
  return `${label}: ${succeeded ? successWord : `not ${successWord}`}`;
}

/**
 * One work item's full ledger history, in order. Each `LedgerEntry` is one
 * dispatch (initial / post_review_repair / review / stuck_loop_gate); each
 * dispatch can carry multiple attempts (retries within that one dispatch)
 * via `entry.attempts`. Real backend/model/duration/tokens/cost per
 * attempt where the backend reported it -- "Unknown" otherwise, never 0.
 */
export function AttemptTimeline({ entries }: { entries: LedgerEntry[] }) {
  if (entries.length === 0) return null;

  return (
    <ol className="space-y-4">
      {entries.map((entry, i) => {
        const reasonKey = entry.dispatch_reason ?? 'initial';
        const ReasonIcon = DISPATCH_REASON_ICON[reasonKey] ?? CircleDot;
        const isReview = reasonKey === 'review';
        const attemptRoutingByNumber = new Map<number, AttemptRoutingRecord>(
          (entry.attempt_routing ?? []).map((attemptRouting: AttemptRoutingRecord) => [
            attemptRouting.attempt_number,
            attemptRouting
          ])
        );

        return (
          <li key={`${entry.timestamp}-${i}`} className="relative pl-8">
            {i < entries.length - 1 && (
              <span className="absolute left-[13px] top-7 bottom-[-16px] w-px bg-subtle" aria-hidden="true" />
            )}
            <span className="absolute left-0 top-0.5 flex items-center justify-center w-7 h-7 rounded-full bg-raised border border-subtle">
              <ReasonIcon size={13} className="text-secondary" aria-hidden="true" />
            </span>

            <div className="card-padded">
              <div className="flex flex-wrap items-center justify-between gap-2 mb-2">
                <span className="text-sm font-semibold text-primary">
                  {DISPATCH_REASON_LABEL[reasonKey] ?? reasonKey}
                </span>
                <span className="text-xs text-muted font-mono">{entry.timestamp}</span>
              </div>

              <div className="flex flex-wrap items-center gap-2 mb-3">
                <span className="text-xs text-secondary">
                  {entry.effective_backend}
                  {entry.effective_model ? `/${entry.effective_model}` : ''}
                </span>
                {isReview ? (
                  <StatusBadge tone={reviewTone(entry.review_verdict)} label={entry.review_verdict ?? 'Unknown verdict'} />
                ) : (
                  <StatusBadge tone={validationTone(entry.validation_result)} label={entry.validation_result ?? 'not run'} />
                )}
                {(entry.reviewer_backend || entry.reviewer_model) && (
                  <span className="text-xs text-secondary">
                    Reviewed by {backendModelLabel(entry.reviewer_backend, entry.reviewer_model)}
                  </span>
                )}
                {entry.human_required && <StatusBadge tone="warning" label="Human required" />}
              </div>

              <div className="grid grid-cols-2 sm:grid-cols-4 gap-x-4 gap-y-2 text-xs">
                <div>
                  <span className="text-muted block">Duration</span>
                  <span className="text-secondary">{formatDuration(entry.duration_seconds)}</span>
                </div>
                <div>
                  <span className="text-muted block">Tokens</span>
                  <span className="text-secondary">{formatTokens(entry.usage.total_tokens)}</span>
                </div>
                <div>
                  <span className="text-muted block">Cost</span>
                  <span className="text-secondary">{formatCost(entry.usage.actual_cost_usd ?? entry.usage.estimated_cost_usd)}</span>
                </div>
                <div>
                  <span className="text-muted block">Attempts</span>
                  <span className="text-secondary">
                    {entry.attempts_completed ?? 0}/{entry.attempts_started ?? 0}
                  </span>
                </div>
                <div>
                  <span className="text-muted block">Diff</span>
                  <span className="text-secondary">
                    {diffLabel(entry.files_changed, 'Files changed')}
                    {' · '}
                    {entry.insertions === null ? 'Insertions: Unknown' : `+${formatCount(entry.insertions)}`}
                    {' / '}
                    {entry.deletions === null ? 'Deletions: Unknown' : `-${formatCount(entry.deletions)}`}
                  </span>
                </div>
                <div>
                  <span className="text-muted block">Commit / Push / MR</span>
                  <div className="mt-1">
                    {commitPushMrBadges(entry)}
                  </div>
                </div>
              </div>

              <p className="mt-2 text-xs text-secondary">
                Repository: {entry.provider} · Model usage: {attributionLabel(entry.usage, entry.effective_model)}
                {entry.usage.backend_instance ? ` · ${entry.usage.backend_instance}` : ''}
                {entry.usage.usage_classification ? ` · ${entry.usage.usage_classification}` : ''}
              </p>
              {renderRoutingDiagnostics(entry.routing_diagnostics, 'Routing rationale', entry.routing_reason, entry.confidence_impact)}
              {attributionUnknownReason(entry.usage) && (
                <p className="mt-1 text-xs text-muted">Attribution: {attributionUnknownReason(entry.usage)}</p>
              )}
              {tokenBreakdown(entry.usage) && (
                <p className="mt-1 text-xs text-muted">Token detail: {tokenBreakdown(entry.usage)}</p>
              )}
              {usageUnknownReasons(entry.usage).map((reason) => (
                <p key={reason} className="mt-1 text-xs text-muted">
                  {reason}
                </p>
              ))}

              {entry.failure_class && (
                <p className="mt-2 text-xs text-critical">
                  Failure: {entry.failure_class}
                  {entry.failure_stage ? ` (${entry.failure_stage})` : ''}
                </p>
              )}

              {isReview && (
                <p className="mt-2 text-xs text-secondary">
                  Supervision: {entry.review_timeout_class ?? 'completed'}
                  {entry.review_idle_timeout_seconds != null ? ` · idle ${entry.review_idle_timeout_seconds}s` : ''}
                  {entry.review_hard_timeout_seconds != null
                    ? ` · hard ${entry.review_hard_timeout_seconds}s`
                    : ' · no hard ceiling'}
                  {entry.review_last_progress_secs != null
                    ? ` · last progress +${Math.round(entry.review_last_progress_secs)}s`
                    : ' · no observed progress'}
                </p>
              )}

              {entry.attempts && entry.attempts.length > 0 && (
                <div className="mt-3 pt-3 border-t border-subtle space-y-2">
                  {entry.attempts.map((attempt: AttemptRecord) => {
                    const attemptRouting = attemptRoutingByNumber.get(attempt.attempt_number);

                    return (
                      <div key={attempt.attempt_number} className="text-xs">
                        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                          <span className="text-muted">#{attempt.attempt_number}</span>
                          <span className="text-secondary">
                            {attributionLabel(attempt.usage, attempt.effective_model)}
                            {attempt.usage.backend_instance ? ` · ${attempt.usage.backend_instance}` : ''}
                          </span>
                          <StatusBadge tone={validationTone(attempt.validation_result)} label={attempt.validation_result ?? 'not run'} />
                          <span className="text-muted">{attempt.usage.usage_classification ?? 'unknown usage'}</span>
                          <span className="text-muted">{formatDuration(attempt.duration_seconds)}</span>
                          <span className="text-muted">{formatCost(attempt.usage.actual_cost_usd ?? attempt.usage.estimated_cost_usd)}</span>
                          {attempt.failure_class && <span className="text-critical">{attempt.failure_class}</span>}
                        </div>
                        {attemptRouting?.routing_diagnostics &&
                          renderRoutingDiagnostics(
                            attemptRouting.routing_diagnostics,
                            'Attempt routing',
                            attemptRouting.routing_diagnostics.human_summary,
                            null
                          )}
                        {attributionUnknownReason(attempt.usage) && (
                          <p className="mt-1 text-muted">Attribution: {attributionUnknownReason(attempt.usage)}</p>
                        )}
                        {tokenBreakdown(attempt.usage) && (
                          <p className="mt-1 text-muted">Token detail: {tokenBreakdown(attempt.usage)}</p>
                        )}
                        {usageUnknownReasons(attempt.usage).map((reason) => (
                          <p key={reason} className="mt-1 text-muted">
                            {reason}
                          </p>
                        ))}
                      </div>
                    );
                  })}
                </div>
              )}

              {entry.mr_url && (
                <a
                  href={entry.mr_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="mt-3 inline-flex items-center gap-1.5 text-xs text-accent hover:underline"
                >
                  <GitPullRequest size={12} aria-hidden="true" />
                  View MR/PR
                </a>
              )}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
