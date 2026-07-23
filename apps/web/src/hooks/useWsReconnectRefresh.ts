import { useEffect, useRef } from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';

/**
 * Re-triggers `refresh` whenever the WebSocket reconnects after having
 * dropped -- a restored connection is otherwise no signal at all that
 * REST-backed panels are current, so a page can sit stale until its own
 * poll timer or a manual navigation happens to fire. Does not fire on the
 * initial connect (the page's own mount effect already covers that).
 */
export function useWsReconnectRefresh(refresh: () => void): void {
  const { reconnectSeq } = useWebSocket();
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  const mountedRef = useRef(false);

  useEffect(() => {
    if (!mountedRef.current) {
      mountedRef.current = true;
      return;
    }
    refreshRef.current();
  }, [reconnectSeq]);
}
