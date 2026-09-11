import { useEffect, useState } from 'react';
import { RefreshCw, WifiOff } from 'lucide-react';
import { isOffline, listenOfflineChange, registerServiceWorker } from '../pwa.js';

/** Issue #534: PWA plumbing UI — offline banner ("read-only, stale") and the
 * update prompt when a new deploy's service worker is waiting. Both render
 * only when relevant; neither blocks the dashboard. */
export function PwaStatusBars() {
  const [offline, setOffline] = useState(isOffline());
  const [updateApply, setUpdateApply] = useState<(() => void) | null>(null);

  useEffect(() => {
    setOffline(isOffline());
    const stop = listenOfflineChange(setOffline);
    registerServiceWorker(setUpdateApply);
    return stop;
  }, []);

  return (
    <>
      {offline && (
        <div
          role="status"
          className="flex items-center gap-2 px-4 py-2 bg-warning/15 text-warning text-sm border-b border-warning/30"
        >
          <WifiOff size={14} aria-hidden="true" />
          Offline — the dashboard is read-only and the data you see may be
          stale. Queued changes are not possible; reconnect to act.
        </div>
      )}
      {updateApply && !offline && (
        <div
          role="status"
          className="flex items-center gap-2 px-4 py-2 bg-raised text-secondary text-sm border-b border-subtle"
        >
          Update available.
          <button
            type="button"
            onClick={() => {
              setUpdateApply(null);
              updateApply();
            }}
            className="inline-flex items-center gap-1.5 px-2 py-1 bg-accent text-white rounded text-xs font-medium hover:bg-accent/90"
          >
            <RefreshCw size={12} aria-hidden="true" />
            Reload to update
          </button>
        </div>
      )}
    </>
  );
}
