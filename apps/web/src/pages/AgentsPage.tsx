import { useEffect, useMemo, useState } from 'react';
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
  const [view, setView] = useState<'activity' | 'capacity' | 'routing'>('activity');
  const status = useGahStore((s) => s.status);
  const quota = useGahStore((s) => s.quota);
  const profiles = useGahStore((s) => s.profiles);
  const profileConfig = useGahStore((s) => s.profileConfig);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const fetchQuota = useGahStore((s) => s.fetchQuota);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const fetchProfileConfig = useGahStore((s) => s.fetchProfileConfig);
  const loopStatus = useGahStore((s) => s.loopStatus);
  const loopAction = useGahStore((s) => s.loopAction);
  const fetchLoopStatus = useGahStore((s) => s.fetchLoopStatus);
  const startLoop = useGahStore((s) => s.startLoop);

  const refresh = (force: boolean) => {
    fetchStatus(profile || undefined, { force });
    fetchQuota({ profile: profile || undefined, since: '7d' }, { force });
    fetchProfiles({ force });
    if (profile) fetchProfileConfig(profile, { force });
    if (profile) fetchLoopStatus(profile, { force });
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
        description="Watch your agents, choose their models, and control how many jobs run at once."
        onRefresh={() => refresh(true)}
        refreshing={status.loading || profiles.loading || profileConfig.loading}
        lastUpdated={oldestFetchedAt(status.fetchedAt, profiles.fetchedAt)}
      />

      {profile && <div className="card-padded flex flex-wrap items-center justify-between gap-3">
        <div>
          <p className="text-sm font-semibold text-primary">{loopStatus.key !== profile || loopStatus.loading ? 'Checking work loop…' : loopStatus.error ? 'Work loop status unavailable' : loopStatus.data?.running ? 'Work loop running' : 'Work loop stopped'}</p>
          <p className="mt-1 text-xs text-muted">{loopStatus.data?.running && loopStatus.key === profile ? 'Agents pick up eligible queued work as capacity becomes available.' : 'Start the work loop to let agents pick up queued jobs. Saving capacity alone does not start it.'}</p>
        </div>
        {loopStatus.key === profile && loopStatus.data && !loopStatus.data.running && !loopStatus.error && <button type="button" className="btn-primary min-h-11" disabled={loopAction.pending || loopStatus.loading} onClick={() => void startLoop(profile)}>{loopAction.pending ? 'Starting…' : 'Start work loop'}</button>}
        {loopAction.error && <p role="alert" className="w-full text-xs text-critical">{loopAction.error}</p>}
      </div>}

      <nav className="flex flex-wrap gap-2 border-b border-subtle pb-3" aria-label="Agent views">
        {([
          ['activity', 'Live activity'],
          ['capacity', 'Models & capacity'],
          ['routing', 'Advanced routing'],
        ] as const).map(([id, label]) => <button key={id} type="button" aria-pressed={view === id}
          onClick={() => setView(id)} className={`min-h-11 rounded-md px-4 py-2 text-sm font-medium ${view === id ? 'bg-accent/15 text-primary ring-1 ring-accent/40' : 'text-secondary hover:bg-white/5'}`}>{label}</button>)}
      </nav>

      {view === 'activity' && <>
      <LiveAgentsCard profile={profile || null} sessions={sessions} controllerRuns={controllerActivity}
        claims={status.data?.active_claims ?? []} candidates={quota.data?.candidates ?? []} factoryAgents={deviceAgents.data?.factory_agents}
        onWatch={(row, watchable) => { if (row.runId) onWatchRun(asRun(row), watchable.map(asRun)); }} />

      <NonFactoryAgentsCard device={deviceAgents.data} deviceError={deviceAgents.error}
        onOpenChat={(chatProfile, sessionId) => { setProfileOverride(chatProfile); updateNavigation({ profile: chatProfile, chat: sessionId }); onNavigate('chat'); }} />
      <p className="text-sm text-muted">Need more agents working at once? <button type="button" className="text-accent underline" onClick={() => setView('capacity')}>Manage models & capacity</button></p>
      </>}

      {view !== 'activity' && (!selected ? (
        <p className="text-sm text-muted">{profiles.loading ? 'Loading the profile…' : 'Pick a project in the navbar to change its agent settings.'}</p>
      ) : view === 'capacity' ? (
        <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-2">
          <AgentLimitsSection key={`models-${profile}`} selectedName={profile} selected={selected} agents={agents} onSaved={() => refresh(true)} />
          <WorkerScalingSection key={`scaling-${profile}`} selectedName={profile} selected={selected} agents={agents} />
        </div>
      ) : config ? <>
        <p className="text-sm text-muted">Choose which agents handle planning, implementation, and review. Use Models & capacity for everyday model and worker changes.</p>
        <AgentPoolSection key={profile} profileName={profile} effective={config} onRefresh={() => fetchProfileConfig(profile, { force: true })} />
      </> : <p role="status" className="text-sm text-muted">{profileConfig.error ? `Couldn’t load routing: ${profileConfig.error}` : 'Loading routing…'}</p>)}
    </div>
  );
}
