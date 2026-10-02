import { useEffect, useState } from 'react';
import type { PairedDevice, PairingAccessRequest } from '@git-agent-harness/contracts';
import { pairingApi } from '../api/client.js';
import { saveCoordinatorToken } from '../api/coordinatorToken.js';

/** The request credential stays in an HttpOnly cookie. Poll only while awaiting
 * a decision, with no overlapping requests or retries after a connection error. */
export function DeviceAccessRequest({ onPaired }: { onPaired: (device: PairedDevice) => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [request, setRequest] = useState<PairingAccessRequest | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    let cancelled = false;
    pairingApi.accessStatus().then(value => {
      if (!cancelled && value.status !== 'claimed') { setRequest(value); setOpen(true); }
    }).catch(() => { /* No request cookie yet. The form can start a request. */ });
    return () => { cancelled = true; };
  }, []);
  useEffect(() => {
    if (!request || !['pending', 'approved'].includes(request.status)) return;
    const expiry = window.setTimeout(() => setRequest(current => current?.id === request.id && ['pending', 'approved'].includes(current.status) ? { ...current, status: 'expired' } : current), Math.max(0, Date.parse(request.expires_at) - Date.now()));
    return () => window.clearTimeout(expiry);
  }, [request?.id, request?.status]);
  useEffect(() => {
    if (!request || request.status !== 'pending' || error) return;
    let cancelled = false;
    const expiresIn = Math.max(0, Date.parse(request.expires_at) - Date.now());
    let timer: number;
    const check = async () => {
      try {
        const value = await pairingApi.accessStatus();
        if (!cancelled) { setRequest(value); if (value.status === 'pending') timer = window.setTimeout(check, 5000); }
      } catch (failure) {
        if (!cancelled) setError(failure instanceof Error ? failure.message : 'Cannot check the request. Check your connection and try again.');
      }
    };
    timer = window.setTimeout(check, Math.min(5000, expiresIn));
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [request?.id, request?.status, error]);
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setError('');
    try { await action(); } catch (failure) { setError(failure instanceof Error ? failure.message : 'Cannot contact the server. Check your connection and try again.'); }
    finally { setBusy(false); }
  };
  return <div className="mt-3">
    <button type="button" className="btn-primary min-h-11" aria-expanded={open} onClick={() => setOpen(!open)}>Request access</button>
    {open && <div className="mt-3 max-w-2xl space-y-3 border-t border-subtle pt-3" aria-label="Access request">
      {error && <p role="alert" className="text-critical">{error}</p>}
      {!request ? <form className="space-y-3" onSubmit={event => { event.preventDefault(); void run(async () => setRequest(await pairingApi.requestAccess(name.trim()))); }}>
        <p className="text-secondary">Ask the owner or a trusted approver to give this device dashboard control.</p>
        <p className="break-all text-secondary">Server: {window.location.origin}</p>
        <label className="block space-y-1">Device name<input required maxLength={80} autoComplete="off" className="input min-h-11 w-full" value={name} onChange={event => setName(event.target.value)} placeholder="My Linux computer" /></label>
        <button className="btn-primary min-h-11" disabled={busy || !name.trim()}>{busy ? 'Sending request…' : 'Send access request'}</button>
      </form> : <>
        <h2 className="text-base font-semibold text-primary">{request.name}</h2>
        <p className="break-all text-secondary">{request.server.name} · {request.server.origin}</p>
        <p className="text-secondary">{request.access}</p>
        <p className="text-secondary">Matching code <strong className="ml-2 font-mono text-base tabular-nums text-primary">{request.matching_code}</strong></p>
        <p className="text-secondary">Expires {new Date(request.expires_at).toLocaleTimeString()}.</p>
        <p role="status" className={request.status === 'approved' ? 'text-good' : 'text-secondary'}>{request.status === 'pending' ? 'Waiting for approval. Open the access request notification on your trusted device, or go to Connection & pairing. Compare this code before approving.' : request.status === 'approved' ? 'Access approved. Continue to sign in on this device.' : request.status === 'denied' ? 'Access denied. Ask the owner before sending another request.' : request.status === 'expired' ? 'This request expired. Send a new request when your approver is ready.' : 'This request was already used. Check your connection settings.'}</p>
        {request.status === 'approved' && <button className="btn-primary min-h-11" disabled={busy} onClick={() => void run(async () => {
          const { device } = await pairingApi.claimAccess();
          saveCoordinatorToken('');
          const session = await pairingApi.session();
          if (session.principal.kind !== 'device' || session.principal.id !== device.id) throw new Error('This browser did not retain its device session. Allow cookies and send a new access request.');
          await onPaired(device);
        })}>{busy ? 'Signing in…' : 'Continue to dashboard'}</button>}
        {error && <button className="btn-secondary min-h-11" disabled={busy} onClick={() => void run(async () => setRequest(await pairingApi.accessStatus()))}>Check request again</button>}
        {(['denied', 'expired', 'claimed'].includes(request.status) || request.status === 'approved' && !!error) && <button className="btn-secondary min-h-11" onClick={() => { setRequest(null); setError(''); }}>Start a new request</button>}
      </>}
    </div>}
  </div>;
}
