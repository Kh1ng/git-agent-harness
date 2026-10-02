import { useEffect, useState } from 'react';
import type { PairingAccessRequest } from '@git-agent-harness/contracts';
import { pairingApi } from '../api/client.js';
import { useWebSocket } from '../ws/WebSocketContext.js';

/** Only owner or explicitly enabled controller principals can read or decide
 * requests. A notification selects a request; it never authorizes approval. */
export function DeviceAccessReview() {
  const { activityRevision, reconnectSeq } = useWebSocket();
  const [requests, setRequests] = useState<PairingAccessRequest[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const selectedId = new URLSearchParams(window.location.search).get('pairingRequest');
  const load = async () => {
    try { setRequests((await pairingApi.accessRequests()).requests); setError(''); }
    catch (failure) { setError(failure instanceof Error ? failure.message : 'Cannot load access requests. Try refreshing.'); }
  };
  useEffect(() => { void load(); }, [activityRevision, reconnectSeq]);
  useEffect(() => {
    const nextExpiry = requests?.reduce((next, request) => Math.min(next, Date.parse(request.expires_at)), Infinity);
    if (!nextExpiry || !Number.isFinite(nextExpiry)) return;
    const timer = window.setTimeout(() => void load(), Math.max(0, nextExpiry - Date.now()) + 100);
    return () => window.clearTimeout(timer);
  }, [requests]);
  const decide = async (request: PairingAccessRequest, action: 'approve' | 'deny') => {
    setBusy(request.id); setError(''); setNotice('');
    try {
      await pairingApi.reviewAccess(request.id, action, request.matching_code);
      setNotice(`${action === 'approve' ? 'Approved' : 'Denied'} ${request.name}.`);
      await load();
    } catch (failure) { setError(failure instanceof Error ? failure.message : 'Cannot review this request. Refresh and try again.'); }
    finally { setBusy(null); }
  };
  return <section className="mt-4 space-y-3 border-t border-subtle pt-3" aria-label="Access requests">
    <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-base font-semibold text-primary">Access requests</h2><button className="btn-secondary min-h-11" onClick={() => void load()}>Refresh requests</button></div>
    <p className="text-secondary">Approve only a device you recognize. Compare its matching code with the requesting screen. This grants dashboard control, without owner administration.</p>
    {error && <p role="alert" className="text-critical">{error}</p>}
    {notice && <p role="status" className="text-good">{notice}</p>}
    {requests === null && !error && <p role="status" className="text-secondary">Loading access requests…</p>}
    {requests?.length === 0 && <p className="text-secondary">No pending access requests.</p>}
    {selectedId && requests && !requests.some(request => request.id === selectedId) && <p role="status" className="text-secondary">The opened request is no longer pending. It may have been approved, denied, or expired.</p>}
    <ul className="space-y-3">{requests?.map(request => <li key={request.id} className={`space-y-2 border-t border-subtle pt-3 ${selectedId === request.id ? 'rounded-md ring-2 ring-accent p-3' : ''}`}>
      <h3 className="break-words font-semibold text-primary">{request.name}</h3>
      <p className="text-secondary">Matching code <strong className="ml-2 font-mono text-base tabular-nums text-primary">{request.matching_code}</strong></p>
      <p className="text-secondary">Expires {new Date(request.expires_at).toLocaleTimeString()}.</p>
      {request.status === 'approved' ? <p role="status" className="text-good">Approved. Waiting for this device to sign in.</p> : <div className="flex flex-wrap gap-2"><button className="btn-primary min-h-11" disabled={busy !== null || Date.parse(request.expires_at) <= Date.now()} onClick={() => void decide(request, 'approve')}>Approve {request.name}</button><button className="btn-secondary min-h-11" disabled={busy !== null} onClick={() => void decide(request, 'deny')}>Deny {request.name}</button></div>}
    </li>)}</ul>
  </section>;
}
