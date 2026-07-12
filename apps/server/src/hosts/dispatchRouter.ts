/**
 * Dispatch Router - Handles routing of session dispatches to different hosts
 * Part of MS-3: Dispatch-to-host routing with load awareness
 */

import { getSessionManager } from '../sessions/SessionManager.js';
import type { HostId } from './HostRegistry.js';

let roundRobinCursor = 0;

export function chooseHost(
  candidates: HostId[],
  strategy: 'least_loaded' | 'round_robin' | 'pinned' | string,
  pinnedHostId?: HostId
): HostId {
  if (candidates.length === 0) {
    return 'local';
  }

  const normalizedStrategy = strategy || 'least_loaded';

  if (normalizedStrategy === 'pinned') {
    if (pinnedHostId) {
      return candidates.includes(pinnedHostId) ? pinnedHostId : 'local';
    }
    return 'local';
  }

  if (normalizedStrategy === 'round_robin') {
    const chosen = candidates[roundRobinCursor % candidates.length];
    roundRobinCursor = (roundRobinCursor + 1) % candidates.length;
    return chosen;
  }

  // Default to 'least_loaded' (fewest active sessions)
  const sessionManager = getSessionManager();
  const activeSessions = sessionManager.getActiveSessions();

  let bestHost = candidates[0];
  let minLoad = Infinity;

  for (const hostId of candidates) {
    const count = activeSessions.filter(s => {
      const sessionHostId = s.hostId || 'local';
      return sessionHostId === hostId;
    }).length;

    if (count < minLoad) {
      minLoad = count;
      bestHost = hostId;
    }
  }

  return bestHost;
}
