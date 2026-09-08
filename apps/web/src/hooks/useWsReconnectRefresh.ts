import { useEffect, useRef } from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';

/** Refresh mounted REST data after a reconnect or credential change succeeds.
 * This includes the first authorized connection when initial reads were rejected.
 * Ordinary initial connections are covered by each page's mount effect. */
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
