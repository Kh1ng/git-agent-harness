import { useEffect, useRef } from 'react';

/**
 * Bounded auto-refresh for a REST-backed panel: calls `refresh` on a fixed
 * interval while the tab is visible (a background tab never fires the
 * tick, so switching away doesn't burn requests), and forces one
 * immediate refresh the moment the tab regains focus so a panel doesn't
 * sit on whatever it showed when the user tabbed away until the next
 * timer tick happens to land.
 */
export function useAutoRefresh(refresh: () => void, intervalMs: number): void {
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'visible') return;
      refreshRef.current();
    }, intervalMs);

    const onVisibility = () => {
      if (document.visibilityState === 'visible') {
        refreshRef.current();
      }
    };
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [intervalMs]);
}
