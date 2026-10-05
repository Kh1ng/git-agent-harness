import { useEffect, useMemo } from 'react';
import type { Page } from '../App.js';
import type { DeviceAgentsSnapshot, Session } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { LiveAgentsCard, liveRowTitle } from '../components/LiveAgentsCard.js';
import type { WatchableRun } from '../components/AgentLiveView.js';
import { NonFactoryAgentsCard } from '../components/NonFactoryAgentsCard.js';
import { AgentLimitsSection } from '../components/AgentLimitsSection.js';
import { WorkerScalingSection } from '../components/WorkerScalingSection.js';
import { AgentPoolSection } from './ProfilePanel.js';
import { updateNavigation } from '../lib/navigationState.js';
import { oldestFetchedAt } from '../lib/format.js';

const AGENTS_REFRESH_MS = 60 * 1000;

/**
 * Agents: what every agent is doing now, and the settings that decide how
 * many run and which models are in the pool.
 */
export function AgentsPage({ sessions, deviceAgents, onNavigate, onWatchRun }: {
  sessions: Session[];
  deviceAgents: { data: DeviceAgentsSnapshot | null; error: string | null };
  onNavigate: (page: Page) => void;
  /** Opens a running job's read-only live view. */
  onWatchRun: (run: WatchableRun, running: WatchableRun[]) => void;
}) {
  const { profile: wsProfile, controllerActivity } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
  const setProfileOverride = useUiStore((s) => s.setProfileOverride);
  const profile = profileOverride ?? wsProfile ?? '';
  const status = useGahStore((s) => s.status);
  const quota = useGahStore((s) => s.quota);
  const profiles = useGahStore((s) => s.profiles);
  const profileConfig = useGahStore((s) => s.profileConfig);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const fetchQuota = useGahStore((s) => s.fetchQuota);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const fetchProfileConfig = useGahStore((s) => s.fetchProfileConfig);

  const refresh = (force: boolean) => {
    fetchStatus(profile || undefined, { force });
    fetchQuota({ profile: profile || undefined, since: '7d' }, { force });
    fetchProfiles({ force });
    if (profile) fetchProfileConfig(profile, { force });
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => refresh(false), [profile]);
  useAutoRefresh(() => refresh(true), AGENTS_REFRESH_MS);
  useWsReconnectRefresh(() => refresh(true));

  const selected = profiles.data?.find((candidate) => candidate.name === profile);
  const config = profileConfig.data?.profile === profile ? profileConfig.data : null;
  // Every backend/model the profile routes to, for the limit and boost pickers.
  const agents = useMemo(() => [...new Set([...(config?.improve_candidates ?? []), ...(config?.review_candidates ?? []), ...(config?.pm_candidates ?? [])]
    .filter((candidate) => candidate.model)
    .map((candidate) => `${candidate.backend}/${candidate.model}`))], [config]);
  const asRun = (row: Parameters<typeof liveRowTitle>[0] & { runId: string | null; mode: string | null }): WatchableRun =>
    ({ runId: row.runId!, title: liveRowTitle(row), subtitle: row.mode });

  return (
    <div className="space-y-6">
      <PageHeader
        title="Agents"
        description="What each agent is doing, how many may run, and which models are in the pool"
        onRefresh={() => refresh(true)}
        refreshing={status.loading || profiles.loading || profileConfig.loading}
        lastUpdated={oldestFetchedAt(status.fetchedAt, profiles.fetchedAt)}
      />

      <LiveAgentsCard profile={profile || null} sessions={sessions} controllerRuns={controllerActivity}
        claims={status.data?.active_claims ?? []} candidates={quota.data?.candidates ?? []} factoryAgents={deviceAgents.data?.factory_agents}
        onWatch={(row, watchable) => { if (row.runId) onWatchRun(asRun(row), watchable.map(asRun)); }} />

      {!selected ? (
        <p className="text-sm text-muted">{profiles.loading ? 'Loading the profile…' : 'Pick a project in the navbar to change its agent settings.'}</p>
      ) : (
        <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-2">
          <AgentLimitsSection selectedName={profile} selected={selected} agents={agents} onSaved={() => refresh(true)} />
          <WorkerScalingSection selectedName={profile} selected={selected} agents={agents} />
        </div>
      )}

      {config && <AgentPoolSection profileName={profile} effective={config} onRefresh={() => fetchProfileConfig(profile, { force: true })} />}

      <NonFactoryAgentsCard device={deviceAgents.data} deviceError={deviceAgents.error}
        onOpenChat={(chatProfile, sessionId) => { setProfileOverride(chatProfile); updateNavigation({ profile: chatProfile, chat: sessionId }); onNavigate('chat'); }} />
    </div>
  );
}
