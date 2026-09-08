import { useState } from 'react';
import { coordinatorToken, saveCoordinatorToken } from '../api/coordinatorToken.js';
import { useWebSocket } from '../ws/WebSocketContext.js';

/** Available before authenticated dashboard data loads, including in the native webview. */
export function CoordinatorConnection() {
  const { trustedLanMode, isConnected } = useWebSocket();
  const [token, setToken] = useState(coordinatorToken);
  const [error, setError] = useState('');
  return <div className="mb-4">
    {trustedLanMode && <p role="status" className="mb-3 rounded-md border border-warning p-3 text-sm text-warning">Trusted-LAN mode is enabled. Live connections may work without a token. Remote dashboard data, node setup, and session operations require an access token.</p>}
    <details className="text-sm" open={!isConnected || undefined}>
      <summary className="cursor-pointer text-secondary">Central access token</summary>
      <form className="mt-2 flex max-w-xl flex-wrap items-end gap-2" onSubmit={event => {
        event.preventDefault();
        try { saveCoordinatorToken(token); setError(''); }
        catch { setError('Cannot save the token in this tab. Check your browser storage settings.'); }
      }}>
        <label className="min-w-0 flex-1 text-xs text-secondary">Access token
          <input type="password" autoComplete="off" className="input mt-1 w-full" value={token} onChange={event => setToken(event.target.value)} />
        </label>
        <button className="btn-secondary" type="submit">Save and reconnect</button>
        <p className="w-full text-xs text-muted">Stored only for this tab’s session. Leave empty for local access.</p>
        {error && <p role="alert" className="w-full text-xs text-critical">{error}</p>}
      </form>
    </details>
  </div>;
}
