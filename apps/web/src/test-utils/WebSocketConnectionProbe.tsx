import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';

export function WebSocketConnectionProbe() {
  const { error, isConnected, isConnecting } = useWebSocket();
  const setProfileOverride = useUiStore((state) => state.setProfileOverride);
  const state = error
    ? `Connection error: ${error}`
    : isConnected
      ? 'Live'
      : isConnecting
      ? 'Connecting'
      : 'Disconnected';
  return (
    <div>
      <span>{state}</span>
      <button type="button" onClick={() => setProfileOverride('replacement')}>Replace connection</button>
    </div>
  );
}
