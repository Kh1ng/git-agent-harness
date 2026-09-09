import { useEffect, useRef, useState } from 'react';
import { coordinatorToken, saveCoordinatorToken, TOKEN_CHANGED_EVENT } from '../api/coordinatorToken.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { DevicePairing } from './DevicePairing.js';

/** Available before authenticated dashboard data loads, including in the native webview. */
export function CoordinatorConnection() {
  const { trustedLanMode, isConnected } = useWebSocket();
  const [token, setToken] = useState(coordinatorToken);
  const [error, setError] = useState('');
  const ownerForm = useRef<HTMLDetailsElement>(null);
  const tokenInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const changed = () => setToken(coordinatorToken());
    window.addEventListener(TOKEN_CHANGED_EVENT, changed);
    return () => window.removeEventListener(TOKEN_CHANGED_EVENT, changed);
  }, []);
  return <div className="mt-3">
    {trustedLanMode && <p role="status" className="mb-3 rounded-md border border-warning p-3 text-sm text-warning">Trusted-LAN mode is enabled. Live connections may work without a token. Remote dashboard data and session operations require pairing or an access token. Node setup requires owner access.</p>}
    <details ref={ownerForm} className="text-sm" open={!isConnected || undefined}>
      <summary className="cursor-pointer text-secondary"><span>Central access token</span> <span className="text-muted">(optional owner access)</span></summary>
      <form className="mt-2 flex max-w-xl flex-wrap items-end gap-2" onSubmit={event => {
        event.preventDefault();
        try { saveCoordinatorToken(token); setError(''); }
        catch { setError('Cannot save the token in this tab. Check your browser storage settings.'); }
      }}>
        <label className="min-w-0 flex-1 text-xs text-secondary">Access token
          <input ref={tokenInput} type="password" autoComplete="off" className="input mt-1 w-full" value={token} onChange={event => setToken(event.target.value)} />
        </label>
        <button className="btn-secondary" type="submit">Save and reconnect</button>
        <p className="w-full text-xs text-muted">Owner access is required to generate pairing QR codes and administer central. Paired devices can use the dashboard without this token. Stored only for this tab’s session.</p>
        {error && <p role="alert" className="w-full text-xs text-critical">{error}</p>}
      </form>
    </details>
    <DevicePairing requestOwnerAccess={() => {
      if (ownerForm.current) ownerForm.current.open = true;
      tokenInput.current?.focus();
    }} />
  </div>;
}
