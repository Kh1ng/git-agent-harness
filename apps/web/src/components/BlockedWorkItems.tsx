import { useState } from 'react';
import { Copy, Check, ExternalLink, Hammer } from 'lucide-react';
import { StatusBadge } from './ui/StatusBadge.js';
import { gahApi, GahApiError } from '../api/client.js';
import type { Blocker } from '@git-agent-harness/contracts';

/** Issue #503: full typed rendering of work-item-scoped blockers — reason
 * code, redacted explanation, attempted backend/model chain, next-eligible
 * time, and the deterministic remediation plan with its safe actions.
 * Mutations offered here are only the ones the underlying policy layer
 * already authorizes (ledger clear-attempts); paid-route grant/revoke and
 * review holds live in their own surfaces. Unknown/legacy reasons stay
 * visibly unknown. */
export function BlockedWorkItems({ blockers }: { blockers: Blocker[] }) {
  return (
    <ul className="space-y-3">
      {blockers.map((b, i) => (
        <BlockedWorkItem key={`blocked-${i}`} blocker={b} />
      ))}
    </ul>
  );
}

function BlockedWorkItem({ blocker }: { blocker: Blocker }) {
  const [copied, setCopied] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const workId = blocker.source_reference;
  const isUnknownReason =
    !blocker.reason_code || blocker.reason_code === 'unknown';

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      setTimeout(() => setCopied((current) => (current === key ? null : current)), 1500);
    } catch {
      setCopied(null);
    }
  };

  const clearAttempts = async () => {
    if (!workId) return;
    setClearing(true);
    setError(null);
    try {
      await gahApi.ledgerClearAttempts({ work_id: workId });
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Clear attempts failed.');
    } finally {
      setClearing(false);
    }
  };

  // A blocker's reference is usually a work id; some carry a full PR/MR URL.
  const prUrl = blocker.source_reference?.startsWith('http')
    ? blocker.source_reference
    : null;
  const plan = blocker.remediation_plan;

  return (
    <li className="border border-subtle rounded-md p-2.5 space-y-2">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="text-sm text-primary">
            {workId ? <span className="font-mono">{workId}</span> : <span className="text-muted">Unknown work item</span>}
            {isUnknownReason ? (
              <StatusBadge tone="warning" label="Unknown reason" />
            ) : (
              <StatusBadge tone="warning" label={blocker.reason_code ?? 'blocked'} />
            )}
          </p>
          {blocker.message && <p className="text-xs text-secondary mt-0.5">{blocker.message}</p>}
          <p className="text-xs text-muted mt-0.5">
            {blocker.backend && (
              <>
                attempted: <span className="font-mono">{blocker.backend}{blocker.model ? `/${blocker.model}` : ''}</span> ·{' '}
              </>
            )}
            {blocker.until && <>next eligible: {blocker.until}</>}
          </p>
        </div>
        {prUrl && (
          <a
            href={prUrl}
            target="_blank"
            rel="noreferrer"
            className="text-xs text-secondary hover:text-primary inline-flex items-center gap-1"
          >
            PR/MR <ExternalLink size={12} aria-hidden="true" />
          </a>
        )}
      </div>

      {error && <p className="text-xs text-critical">{error}</p>}

      {plan ? (
        <div className="text-xs space-y-1">
          <p className="text-muted">
            Required authority: <span className="text-secondary">{authorityLabel(plan.required_authority)}</span>
          </p>
          <ul className="space-y-1">
            {plan.safe_actions.map((action, index) => (
              <li key={`action-${index}`} className="flex items-start gap-2">
                <span className="text-muted mt-0.5">{index + 1}.</span>
                <div className="min-w-0">
                  <p className="text-secondary">{action.summary}</p>
                  {action.command && (
                    <button
                      type="button"
                      onClick={() => copy(action.command ?? '', `cmd-${index}`)}
                      title="Copy command"
                      className="inline-flex items-center gap-1.5 font-mono text-xs bg-raised border border-subtle rounded px-2 py-0.5 text-primary hover:border-accent"
                    >
                      {copied === `cmd-${index}` ? <Check size={11} aria-hidden="true" /> : <Copy size={11} aria-hidden="true" />}
                      {action.command}
                    </button>
                  )}
                  {action.api_action && <p className="text-muted">{action.api_action}</p>}
                </div>
              </li>
            ))}
          </ul>
          {plan.result === 'no_automatic_remediation' && plan.reason && (
            <p className="text-muted">{plan.reason}</p>
          )}
          {plan.result === 'plan' &&
            plan.reason_code === 'external_api_approval_required' &&
            workId && (
              <p className="text-muted">
                The grant releases this hold automatically; the loop re-selects the work.
              </p>
            )}
          {plan.result === 'plan' &&
            workId &&
            (plan.reason_code === 'retry_budget_exhausted' ||
              plan.reason_code === 'fix_retry_cap_exceeded') && (
              <button
                type="button"
                onClick={clearAttempts}
                disabled={clearing}
                className="inline-flex items-center gap-1.5 px-2.5 py-1 bg-accent text-white rounded-md text-xs font-medium hover:bg-accent/90 disabled:opacity-50"
              >
                <Hammer size={12} aria-hidden="true" />
                {clearing ? 'Clearing…' : 'Clear attempts & retry'}
              </button>
            )}
        </div>
      ) : (
        <p className="text-xs text-muted">No remediation plan for this reason code.</p>
      )}
    </li>
  );
}

function authorityLabel(authority: string): string {
  return authority.replace(/_/g, ' ');
}


