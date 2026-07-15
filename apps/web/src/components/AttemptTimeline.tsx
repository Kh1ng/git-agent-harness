import { GitBranch, GitPullRequest, Eye, RotateCw, ShieldAlert, CircleDot } from 'lucide-react';
import type { LedgerEntry, LedgerUsage } from '@git-agent-harness/contracts';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { formatCost, formatDuration, formatTokens } from '../lib/format.js';

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
              </div>

              <p className="mt-2 text-xs text-secondary">
                Repository: {entry.provider} · Model usage: {attributionLabel(entry.usage, entry.effective_model)}
                {entry.usage.backend_instance ? ` · ${entry.usage.backend_instance}` : ''}
                {entry.usage.usage_classification ? ` · ${entry.usage.usage_classification}` : ''}
              </p>
              {attributionUnknownReason(entry.usage) && (
                <p className="mt-1 text-xs text-muted">Attribution: {attributionUnknownReason(entry.usage)}</p>
              )}

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
                  {entry.attempts.map((attempt) => (
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
                      {attributionUnknownReason(attempt.usage) && (
                        <p className="mt-1 text-muted">Attribution: {attributionUnknownReason(attempt.usage)}</p>
                      )}
                    </div>
                  ))}
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
