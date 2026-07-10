import { Wifi, WifiOff, Loader2 } from 'lucide-react';

type ConnectionStatusProps = {
  isConnected: boolean;
  isConnecting: boolean;
  error: string | null;
  serverVersion: string | null;
};

/** Compact connection indicator, not a full-width banner -- it only needs
 * to be loud when something is actually wrong (disconnected/erroring);
 * "connected" should be a quiet, glanceable fact, not a persistent alert. */
export function ConnectionStatus({ isConnected, isConnecting, error, serverVersion }: ConnectionStatusProps) {
  if (isConnecting) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs text-muted">
        <Loader2 size={12} className="animate-spin" aria-hidden="true" />
        Connecting…
      </span>
    );
  }

  if (error || !isConnected) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs text-critical" role="status">
        <WifiOff size={12} aria-hidden="true" />
        {error ? `Connection error: ${error}` : 'Disconnected'}
      </span>
    );
  }

  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-muted" role="status">
      <Wifi size={12} className="text-good" aria-hidden="true" />
      Live{serverVersion && <span className="text-muted">· v{serverVersion}</span>}
    </span>
  );
}
