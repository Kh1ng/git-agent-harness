import { useEffect, useRef, useState } from 'react';
import { RefreshCw, X } from 'lucide-react';
import { gahApi, GahApiError } from '../api/client.js';
import type { AdminUpdateState, ReleaseChannelStatus } from '@git-agent-harness/contracts';

const RELEASE_POLL_MS = 15 * 60_000;
const UPDATE_STATUS_POLL_MS = 2_000;

/**
 * Issue #1416: non-blocking "Update available · vX → vY · Restart to update"
 * banner at the top of the dashboard, beside the PWA status bars. Checking is
 * periodic, not per-render; clicking "Restart to update" starts the release
 * install (`--from-release`, no rebuild) and follows the existing
 * admin-update state file across the restart it triggers, reloading the page
 * once the new version is serving. Renders nothing when the server has the
 * admin update feature disabled (GAH_ENABLE_ADMIN_UPDATE unset) or the
 * channel has nothing newer.
 */
export function GahUpdateBanner() {
  const [enabled, setEnabled] = useState(true);
  const [release, setRelease] = useState<ReleaseChannelStatus | null>(null);
  const [status, setStatus] = useState<AdminUpdateState | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const polling = useRef(false);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      gahApi
        .getReleaseStatus()
        .then((data) => { if (!cancelled) { setRelease(data); setError(null); setEnabled(true); } })
        .catch((err) => {
          if (cancelled) return;
          if (err instanceof GahApiError && err.status === 404) {
            setEnabled(false);
            return;
          }
          // Channel unreachable (offline, GitHub hiccup): stay quiet rather
          // than nagging about a feed that cannot be read.
          setError(err instanceof Error ? err.message : String(err));
        });
    };
    load();
    const timer = setInterval(load, RELEASE_POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    gahApi.getAdminUpdateStatus().then(setStatus).catch(() => undefined);
  }, []);

  const restartToUpdate = async () => {
    if (polling.current) return;
    setStarting(true);
    setError(null);
    try {
      const state = await gahApi.startAdminUpdate('release');
      setStatus(state);
      if (state.status === 'running') {
        polling.current = true;
        const tick = async () => {
          try {
            const next = await gahApi.getAdminUpdateStatus();
            setStatus(next);
            if (next.status === 'running') {
              setTimeout(tick, UPDATE_STATUS_POLL_MS);
            } else {
              polling.current = false;
              if (next.status === 'success' || next.status === 'inferred_restart') {
                window.location.reload();
              }
            }
          } catch {
            // The restart step briefly takes the server down -- keep
            // polling until it comes back and reports a terminal state.
            setTimeout(tick, UPDATE_STATUS_POLL_MS);
          }
        };
        setTimeout(tick, UPDATE_STATUS_POLL_MS);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStarting(false);
    }
  };

  if (!enabled || dismissed || !release?.update_available) return null;

  const running = status?.status === 'running';
  const current = release.current_version;
  const latest = release.latest_version ?? 'newer';

  return (
    <div
      role="status"
      className="flex shrink-0 flex-wrap items-center gap-2 border-b border-accent/30 bg-accent/10 px-4 py-2 text-sm text-primary lg:col-span-2"
    >
      <RefreshCw size={14} aria-hidden="true" className="text-accent" />
      <span className="font-medium">Update available · v{current} → v{latest}</span>
      {running && <span className="text-secondary">Installing release and restarting…</span>}
      {!running && (
        <button
          type="button"
          onClick={() => void restartToUpdate()}
          disabled={starting}
          className="inline-flex items-center gap-1.5 rounded bg-accent-fill px-2 py-1 text-xs font-medium text-white hover:bg-accent-fill/90 disabled:opacity-50"
        >
          {starting ? 'Starting…' : 'Restart to update'}
        </button>
      )}
      <a className="text-xs text-accent underline underline-offset-2" href="#page=settings" onClick={() => setDismissed(true)}>
        Details in Settings
      </a>
      <button
        type="button"
        aria-label="Dismiss update banner"
        className="ml-auto text-secondary hover:text-primary"
        onClick={() => setDismissed(true)}
      >
        <X size={14} aria-hidden="true" />
      </button>
      {error && <span className="text-xs text-critical">{error}</span>}
    </div>
  );
}
