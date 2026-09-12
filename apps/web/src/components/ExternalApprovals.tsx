import { useEffect, useState } from 'react';
import type { ExternalApprovalDecisionScope, ExternalApprovalScope } from '@git-agent-harness/contracts';
import { externalApprovalApi, GahApiError, pairingApi } from '../api/client.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';

const identity = (scope: ExternalApprovalScope) => JSON.stringify([scope.work_id, scope.credential_label, scope.operation_kind]);

export function ExternalApprovals({ profile }: { profile: string }) {
  const [approvals, setApprovals] = useState<ExternalApprovalScope[]>([]);
  const [owner, setOwner] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<{ key: string; action: 'grant' | 'deny' | 'revoke' } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const refresh = async () => {
    setLoading(true);
    try {
      const [rows, session] = await Promise.all([externalApprovalApi.list(profile), pairingApi.session()]);
      setApprovals(rows); setOwner(session.principal.kind === 'owner'); setError(null);
    } catch (failure) {
      if (failure instanceof GahApiError && failure.status === 404 && failure.endpoint === '/api/external-approvals') {
        setApprovals([]); setNotice(null); setError(null); setConfirming(null);
      } else setError(failure instanceof Error ? failure.message : 'Cannot load external approvals.');
    } finally { setLoading(false); }
  };
  useEffect(() => { void refresh(); }, [profile]);
  useAutoRefresh(() => { if (!busy && !confirming) void refresh(); }, 30_000);
  useWsReconnectRefresh(() => { if (!busy) void refresh(); });

  const change = async (approval: ExternalApprovalScope, action: 'grant' | 'deny' | 'revoke') => {
    setBusy(true); setError(null); setNotice(null);
    const scope: ExternalApprovalDecisionScope = { profile: approval.profile, work_id: approval.work_id, credential_label: approval.credential_label, operation_kind: approval.operation_kind };
    try {
      setApprovals(await externalApprovalApi.change(action, scope));
      setConfirming(null);
      setNotice(`${action === 'grant' ? 'Approved' : action === 'deny' ? 'Denied' : 'Revoked'} ${approval.credential_label} for ${approval.work_id}.`);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'Refresh approval status before trying again.');
      setConfirming(null);
    } finally { setBusy(false); }
  };
  const row = (approval: ExternalApprovalScope) => {
    const key = identity(approval);
    const cap = [approval.max_requests == null ? null : `${approval.max_requests} request${approval.max_requests === 1 ? '' : 's'}`, approval.max_dollars == null ? null : `$${approval.max_dollars}`].filter(Boolean).join(' · ') || 'No configured cap';
    return <li key={key} className="py-4 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1 break-words [overflow-wrap:anywhere]">
          <p className="font-semibold text-primary">{approval.work_id} · {approval.credential_label}</p>
          <p className="text-sm text-secondary">{approval.purpose ?? approval.operation_kind}</p>
          <p className="text-xs text-muted">{cap} · expires {approval.expires_at ?? 'never'} · used {approval.consumed_requests}</p>
          {approval.denial_reason && <p className="mt-1 text-xs text-critical">{approval.denial_reason}</p>}
        </div>
        {owner && approval.state === 'requested' && !confirming && <div className="flex gap-2">
          <button className="btn-primary min-h-11" disabled={busy || loading || !!error} onClick={() => setConfirming({ key, action: 'grant' })}>Review approval</button>
          <button className="btn-secondary min-h-11" disabled={busy || loading || !!error} onClick={() => setConfirming({ key, action: 'deny' })}>Deny</button>
        </div>}
        {owner && approval.active && !confirming && <button className="btn-secondary min-h-11" disabled={busy || loading || !!error} onClick={() => setConfirming({ key, action: 'revoke' })}>Revoke approval</button>}
      </div>
      {owner && confirming?.key === key && <div className="mt-3 max-w-prose space-y-3 text-sm">
        <p className="text-secondary">{confirming.action === 'grant' ? `Allow only the recorded ${cap} scope for this work item?` : confirming.action === 'deny' ? 'Deny this request and keep the work item held?' : 'Stop future credential injection for this work item?'}</p>
        <div className="flex flex-wrap gap-2">
          <button className={confirming.action === 'grant' ? 'btn-primary min-h-11' : 'btn-secondary min-h-11'} disabled={busy || !!error} onClick={() => void change(approval, confirming.action)}>{busy ? 'Saving…' : confirming.action === 'grant' ? 'Approve exact scope' : confirming.action === 'deny' ? 'Confirm deny' : 'Confirm revoke'}</button>
          <button className="btn-secondary min-h-11" disabled={busy} onClick={() => setConfirming(null)}>Cancel</button>
        </div>
      </div>}
    </li>;
  };
  const pending = approvals.filter(approval => approval.state === 'requested');
  const active = approvals.filter(approval => approval.active);
  const history = approvals.filter(approval => approval.state !== 'requested' && !approval.active);
  if (!loading && !error && approvals.length === 0 && !notice) return null;
  return <section className="card-padded" aria-label="External API approvals">
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <h3 className="text-sm font-semibold text-primary">External API approvals</h3>
      <button aria-label="Refresh external approvals" className="min-h-11 text-xs text-secondary hover:text-primary" disabled={loading || busy} onClick={() => void refresh()}>{loading ? 'Loading…' : 'Refresh'}</button>
    </div>
    {error && <p role="alert" className="mb-3 text-sm text-critical">{error}</p>}
    {notice && <p role="status" className="mb-3 text-sm text-secondary">{notice}</p>}
    {!owner && (pending.length > 0 || active.length > 0) && <p className="mb-4 text-sm text-muted">Owner access is required to decide or revoke external API use. Sign in under Settings → Connection & pairing.</p>}
    {pending.length > 0 && <ul className="divide-y divide-subtle">{pending.map(row)}</ul>}
    {active.length > 0 && <details className={pending.length ? 'mt-4 border-t border-subtle pt-4' : ''} open={!pending.length}>
      <summary className="min-h-11 cursor-pointer text-sm font-medium text-secondary">Active approvals ({active.length})</summary>
      <ul className="mt-3 divide-y divide-subtle">{active.map(row)}</ul>
    </details>}
    {history.length > 0 && <details className="mt-4 border-t border-subtle pt-4">
      <summary className="min-h-11 cursor-pointer text-sm font-medium text-secondary">Resolved approvals ({history.length})</summary>
      <ul className="mt-3 divide-y divide-subtle">{history.map(row)}</ul>
    </details>}
  </section>;
}
