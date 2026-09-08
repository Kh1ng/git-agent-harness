import { useEffect, useState } from 'react';
import type { ChatNodeInfo } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

/** Reads cached node eligibility for one project/backend. The parent supplies
 * its fleet/reconnect key; no polling or live worker checks run in the UI. */
export function useChatNodes(profile: string, backend: string | null, enabled: boolean, refreshKey: string) {
  const key = `${profile}\0${backend ?? ''}\0${refreshKey}`;
  const [snapshot, setSnapshot] = useState<{ key: string; nodes: ChatNodeInfo[]; error: string | null } | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    setSnapshot(null);
    gahApi.getChatNodes(profile, backend ?? undefined)
      .then(({ nodes }) => { if (!cancelled) setSnapshot({ key, nodes, error: null }); })
      .catch(() => { if (!cancelled) setSnapshot({ key, nodes: [], error: 'Could not load available nodes.' }); });
    return () => { cancelled = true; };
  }, [profile, backend, enabled, key, attempt]);
  const current = enabled && snapshot?.key === key ? snapshot : null;
  return { nodes: current?.nodes ?? [], loading: enabled && !current, error: current?.error ?? null, retry: () => setAttempt(value => value + 1) };
}
