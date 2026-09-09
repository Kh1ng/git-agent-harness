import { useEffect, useState } from 'react';
import type { PaidRouteApproval, PaidRouteScope } from '@git-agent-harness/contracts';
import { paidRouteApi, pairingApi } from '../api/client.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';

/** Work-scoped spending decisions stay beside work; approved routes remain
 * available for revocation after their original blocker disappears. */
export function PaidRouteApprovals({ profile }: { profile: string }) {
  const [routes, setRoutes] = useState<PaidRouteApproval[]>([]);
  const [owner, setOwner] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const refresh = async () => {
    setLoading(true);
    try {
      const [rows, session] = await Promise.all([paidRouteApi.list(profile), pairingApi.session()]);
      setRoutes(rows); setOwner(session.principal.kind === 'owner'); setError(null);
    } catch (failure) { setError(failure instanceof Error ? failure.message : 'Cannot load paid-route approvals.'); }
    finally { setLoading(false); }
  };
  useEffect(() => { void refresh(); }, [profile]);
  useAutoRefresh(() => { if (!busy && !confirming) void refresh(); }, 30_000);
  useWsReconnectRefresh(() => { if (!busy) void refresh(); });

  const change = async (route: PaidRouteApproval) => {
    setBusy(true); setError(null); setNotice(null);
    const action = route.approved ? 'revoke' : 'grant';
    const scope: PaidRouteScope = { profile: route.profile, work_id: route.work_id, backend: route.backend, backend_instance: route.backend_instance, model: route.model };
    try {
      setRoutes(await paidRouteApi.change(action, scope));
      setConfirming(null);
      setNotice(`${action === 'grant' ? 'Approved paid use' : 'Revoked paid use'} for ${route.work_id}.`);
    } catch (failure) {
      // An unknown outcome must be read back before another spend decision.
      setError(failure instanceof Error ? failure.message : 'Refresh approval status before trying again.');
      setConfirming(null);
    } finally { setBusy(false); }
  };
  const row = (route: PaidRouteApproval) => {
    const key = JSON.stringify([route.work_id, route.backend, route.backend_instance, route.model]);
    return <li key={key} className="py-4 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1 break-words [overflow-wrap:anywhere]">
          <p className="font-semibold text-primary">{route.work_id}</p>
          <p className="text-sm text-secondary">{route.backend} · {route.model ?? 'Default model'}</p>
          <p className="text-xs text-muted">Account: {route.backend_instance ?? 'Default backend account'}</p>
        </div>
        {owner && confirming !== key && <button className="btn-secondary min-h-11" disabled={busy || loading || !!error} onClick={() => setConfirming(key)}>{route.approved ? 'Revoke approval' : 'Review approval'}</button>}
      </div>
      {owner && confirming === key && <div className="mt-3 max-w-prose space-y-3 text-sm">
        <p className="text-secondary">{route.approved ? 'Stop allowing future paid dispatches on this route for this work item? Running attempts will not be stopped.' : 'Allow paid dispatches on this exact route for this work item? Approval stays active until revoked and does not set a spend limit.'}</p>
        <div className="flex flex-wrap gap-2">
          <button className="btn-primary min-h-11" disabled={busy || !!error} onClick={() => void change(route)}>{busy ? 'Saving…' : route.approved ? 'Confirm revoke' : 'Approve paid use'}</button>
          <button className="btn-secondary min-h-11" disabled={busy} onClick={() => setConfirming(null)}>Cancel</button>
        </div>
      </div>}
    </li>;
  };
  const requested = routes.filter(route => route.requested && !route.approved);
  const approved = routes.filter(route => route.approved);
  if (!loading && !error && routes.length === 0 && !notice) return null;
  return <section className="card-padded" aria-label="Paid-route approvals">
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <h3 className="text-sm font-semibold text-primary">Paid-route approvals</h3>
      <button aria-label="Refresh approvals" className="min-h-11 text-xs text-secondary hover:text-primary" disabled={loading || busy} onClick={() => void refresh()}>{loading ? 'Loading…' : 'Refresh'}</button>
    </div>
    {error && <p role="alert" className="mb-3 text-sm text-critical">{error}</p>}
    {notice && <p role="status" className="mb-3 text-sm text-secondary">{notice}</p>}
    {!owner && routes.length > 0 && <p className="mb-4 text-sm text-muted">Owner access is required to approve or revoke paid use. Sign in under Settings → Connection & pairing.</p>}
    {requested.length > 0 && <ul className="divide-y divide-subtle">{requested.map(row)}</ul>}
    {approved.length > 0 && <details className={requested.length ? 'mt-4 border-t border-subtle pt-4' : ''}>
      <summary className="min-h-11 cursor-pointer text-sm font-medium text-secondary">Approved routes ({approved.length})</summary>
      <ul className="mt-3 divide-y divide-subtle">{approved.map(row)}</ul>
    </details>}
  </section>;
}
