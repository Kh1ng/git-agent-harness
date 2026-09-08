import { useEffect, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import type { PairedDevice, PairingOffer, PairingPreview } from '@git-agent-harness/contracts';
import { GahApiError, pairingApi } from '../api/client.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { saveCoordinatorToken } from '../api/coordinatorToken.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';

function pairingLink(value: string): { origin: string; code: string; serverId: string } {
  const url = new URL(value);
  const fragment = new URLSearchParams(url.hash.slice(1));
  const code = fragment.get('pair') ?? '';
  const serverId = fragment.get('server') ?? '';
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || !/^[A-Za-z0-9_-]{32}$/.test(code) || !/^[0-9a-f-]{36}$/.test(serverId)) throw new Error('Paste the complete pairing link shown by the owner.');
  return { origin: url.origin, code, serverId };
}

/** Same-origin browser pairing. The server sets an HttpOnly session cookie;
 * neither this component nor the QR renderer receives a device credential. */
export function DevicePairing() {
  const { isConnected } = useWebSocket();
  const [pending, setPending] = useState(() => {
    try { return pairingLink(window.location.href); } catch { return null; }
  });
  const [preview, setPreview] = useState<PairingPreview | null>(null);
  const [principal, setPrincipal] = useState<'owner' | 'device' | null>(null);
  const [open, setOpen] = useState(Boolean(pending));
  const [origin, setOrigin] = useState(window.location.origin);
  const [manualLink, setManualLink] = useState('');
  const [deviceName, setDeviceName] = useState('');
  const [devices, setDevices] = useState<PairedDevice[] | null>(null);
  const [offer, setOffer] = useState<PairingOffer | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const loadSession = () => { pairingApi.session().then(({ principal }) => setPrincipal(principal.kind)).catch(error => {
      if (error instanceof GahApiError && [401, 403].includes(error.status)) {
        if (principal === 'device') { setNotice(''); setError(error.message); }
        setPrincipal(null);
      }
    }); };
  useEffect(loadSession, [isConnected]);
  useWsReconnectRefresh(loadSession);
  useEffect(() => {
    if (!pending) return;
    // Fragments never reach the HTTP server; remove them from browser history too.
    if (window.location.hash) window.history.replaceState(null, '', window.location.pathname + window.location.search);
    let cancelled = false;
    setPreview(null);
    setError('');
    pairingApi.inspect(pending.code, pending.serverId)
      .then(value => { if (!cancelled) setPreview(value); })
      .catch(error => { if (!cancelled) setError(error instanceof Error ? error.message : 'Cannot contact the pairing server.'); });
    return () => { cancelled = true; };
  }, [pending]);
  const loadDevices = () => { pairingApi.devices().then(({ devices }) => setDevices(devices)).catch(error => setError(String(error.message))); };
  useEffect(() => { if (open && principal === 'owner') loadDevices(); }, [open, principal]);
  useEffect(() => {
    if (!offer) return;
    const timeout = setTimeout(() => { setOffer(null); setNotice('Pairing code expired. Generate a new code.'); }, Math.max(0, Date.parse(offer.expires_at) - Date.now()));
    return () => clearTimeout(timeout);
  }, [offer]);
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setError(''); setNotice('');
    try { await action(); } catch (error) { setError(error instanceof Error ? error.message : 'Pairing failed. Try again.'); }
    finally { setBusy(false); }
  };
  const url = offer ? `${offer.server.origin}/#pair=${offer.code}&server=${offer.server.id}` : '';
  return <section className="mt-3 text-sm max-sm:[&_button]:min-h-11 max-sm:[&_input]:min-h-11" aria-label="Device pairing">
    <button type="button" className="btn-secondary" aria-expanded={open} onClick={() => setOpen(!open)}>{principal === 'owner' ? 'Pair a device' : 'Pair this device'}</button>
    {open && <div className="mt-3 max-w-2xl space-y-4 rounded-md border border-subtle bg-raised p-4">
      {error && <p role="alert" className="text-critical">{error}</p>}
      {notice && <p role="status" className="text-secondary">{notice}</p>}
      {pending && preview ? <form className="space-y-3" onSubmit={event => { event.preventDefault(); void run(async () => {
        const { device } = await pairingApi.redeem(pending.code, pending.serverId, deviceName);
        saveCoordinatorToken('');
        const session = await pairingApi.session();
        if (session.principal.kind !== 'device' || session.principal.id !== device.id) throw new Error('This browser did not retain its device session. Allow cookies and ask the owner for a new pairing code.');
        setPending(null); setPreview(null); setPrincipal('device'); setNotice(`Paired as ${device.name}. Access expires ${new Date(device.expires_at).toLocaleDateString()}.`);
      }); }}>
        <h2 className="text-base font-semibold text-primary">Confirm this server</h2>
        <p className="break-all text-primary">{preview.server.name} · {preview.server.origin}</p>
        <p className="break-all text-secondary">Server ID: {preview.server.id}</p>
        <p className="text-secondary">{preview.access}</p>
        {preview.server.origin.startsWith('http:') && <p className="text-warning">This server permits unencrypted HTTP. Pair only on your trusted LAN or secure tunnel.</p>}
        <label className="block space-y-1">Device name<input className="input w-full" required maxLength={80} autoComplete="off" value={deviceName} onChange={event => setDeviceName(event.target.value)} placeholder="My phone" /></label>
        <div className="flex flex-wrap gap-2">
          <button className="btn-primary" disabled={busy || !deviceName.trim()}>Confirm server and pair</button>
          <button type="button" className="btn-secondary" onClick={() => { setPending(null); setPreview(null); }}>Cancel pairing</button>
        </div>
      </form> : pending && !error ? <p role="status">Checking pairing code…</p> : null}
      {principal === 'owner' && !pending && <>
        <form className="space-y-3" onSubmit={event => { event.preventDefault(); void run(async () => setOffer(await pairingApi.create(origin))); }}>
          <p className="text-secondary">Give a trusted device dashboard control. Codes expire after five minutes and work once.</p>
          <label className="block space-y-1">Central server address<input className="input w-full" type="url" required value={origin} onChange={event => setOrigin(event.target.value)} /></label>
          <p className="text-xs text-secondary">Use an address the other device can reach. Do not use localhost for another computer or phone.</p>
          <button className="btn-primary" disabled={busy}>Generate pairing code</button>
        </form>
        {offer && <div className="space-y-2">
          <p className="break-all text-primary">{offer.server.name} · {offer.server.origin}</p>
          <p className="break-all text-secondary">Server ID: {offer.server.id}</p>
          <p className="text-secondary">{offer.access}</p>
          <QRCodeSVG value={url} size={192} marginSize={4} title="Scan to pair with this central server" />
          <p className="text-secondary">Scan with the device’s camera, or open the pairing link. Expires {new Date(offer.expires_at).toLocaleTimeString()}.</p>
          <label className="block space-y-1">Pairing link<input className="input w-full" readOnly value={url} onFocus={event => event.target.select()} /></label>
        </div>}
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-2"><h2 className="font-semibold text-primary">Paired devices</h2><button type="button" className="btn-secondary" onClick={loadDevices}>Refresh devices</button></div>
          {devices?.length === 0 && <p className="text-secondary">No paired devices.</p>}
          <ul className="space-y-2">{devices?.map(device => <li key={device.id} className="flex flex-wrap items-center justify-between gap-2 border-t border-subtle pt-2">
            <span className="min-w-0 break-words">{device.name} <span className="text-secondary">· {device.revoked_at ? 'Revoked' : Date.parse(device.expires_at) <= Date.now() ? 'Expired' : `Expires ${new Date(device.expires_at).toLocaleDateString()}`}</span></span>
            {!device.revoked_at && <button type="button" className="btn-secondary" disabled={busy} onClick={() => void run(async () => { await pairingApi.revoke(device.id); loadDevices(); setNotice(`Revoked ${device.name}. Existing device connections are closed.`); })}>Revoke {device.name}</button>}
          </li>)}</ul>
        </div>
      </>}
      {principal === 'device' && !pending && <div className="space-y-2"><p className="text-secondary">This browser has a paired device session. The owner can revoke it individually.</p><button type="button" className="btn-secondary" disabled={busy} onClick={() => void run(async () => { await pairingApi.logout(); saveCoordinatorToken(''); setPrincipal(null); setNotice('Device session cleared from this browser.'); })}>Disconnect this browser</button></div>}
      {!pending && <form className="space-y-2 border-t border-subtle pt-3" onSubmit={event => { event.preventDefault(); try {
        const parsed = pairingLink(manualLink);
        if (parsed.origin !== window.location.origin) { window.location.assign(`${parsed.origin}/#pair=${parsed.code}&server=${parsed.serverId}`); return; }
        setPending(parsed); setError('');
      } catch (error) { setError(error instanceof Error ? error.message : 'Invalid pairing link.'); } }}>
        <label className="block space-y-1">Open a pairing link<input className="input w-full" type="url" required value={manualLink} onChange={event => setManualLink(event.target.value)} placeholder="Paste the server address and pairing link" /></label>
        <button className="btn-secondary" disabled={busy}>Review pairing link</button>
      </form>}
      {pending && error && <button type="button" className="btn-secondary" onClick={() => { setPending(null); setPreview(null); setError(''); }}>Use another pairing link</button>}
    </div>}
  </section>;
}
