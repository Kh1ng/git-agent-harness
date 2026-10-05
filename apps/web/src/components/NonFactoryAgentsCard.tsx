import { useEffect, useState } from 'react';
import { MessageSquare, Terminal } from 'lucide-react';
import type { ChatSessionProjectGroup, DeviceAgentsSnapshot } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { agentDisplayName, formatDuration, modelDisplayName } from './LiveAgentsCard.js';

const REFRESH_MS = 15_000;
/** A dashboard chat counts as in use for this long after its last turn. */
const CHAT_ACTIVE_MS = 30 * 60_000;

/** A transcript written this recently means the agent is mid-turn. */
const WORKING_MS = 90_000;

const home = (path: string) => path.replace(/^\/(home|Users)\/[^/]+/, '~');

/**
 * Agents at work outside the factory: claude, codex and similar CLIs
 * running on this device in a directory the factory does not own, and
 * dashboard chats with a turn in the last half hour. They draw on the same
 * subscriptions as the factory, so they explain usage the Live card cannot.
 */
export function NonFactoryAgentsCard({ device, deviceError, onOpenChat }: {
  /** The device scan, fetched once by the shell for the navbar, the Live card and this panel. */
  device: DeviceAgentsSnapshot | null;
  deviceError: string | null;
  onOpenChat: (profile: string, sessionId: string) => void;
}) {
  const [chats, setChats] = useState<ChatSessionProjectGroup[] | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const refresh = () => {
    gahApi.getAllChatSessions().then(({ projects }) => setChats(projects)).catch(() => { /* Chats stay as last seen. */ });
    setNow(Date.now());
  };
  useEffect(refresh, []);
  useAutoRefresh(refresh, REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const activeChats = (chats ?? []).flatMap((group) => group.sessions
    .filter((session) => session.outcome === 'live' && now - session.lastActiveAt < CHAT_ACTIVE_MS)
    .map((session) => ({ profile: group.profile, session })))
    .sort((a, b) => b.session.lastActiveAt - a.session.lastActiveAt);
  const agents = device?.agents ?? [];
  const total = agents.length + activeChats.length;

  return (
    <section className="card-padded" aria-labelledby="non-factory-agents-title">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h3 id="non-factory-agents-title" className="flex items-center gap-2 text-sm font-semibold text-primary"
          title="Agents on this device and dashboard chats in use right now, outside the Factory">
          <Terminal size={15} className="text-accent" aria-hidden="true" />
          Non Factory Agents
        </h3>
        <span className="text-xs tabular-nums text-muted">{total > 0 ? `${total} open` : ''}</span>
      </div>
      {deviceError && !device && <p className="mb-2 text-xs text-muted">Device agents need a server with this feature: {deviceError}</p>}
      {device && !device.supported && <p className="mb-2 text-xs text-muted">This server's operating system cannot list device agents yet.</p>}
      {total === 0 ? (
        <p className="text-sm text-muted">{device === null && chats === null && !deviceError ? 'Looking for agents…' : 'No agent is running outside the Factory.'}</p>
      ) : (
        <ul className="divide-y divide-subtle" aria-label="Non factory agents">
          {agents.map((agent) => {
            const idleMs = agent.last_activity_at ? now - Date.parse(agent.last_activity_at) : null;
            const working = idleMs !== null && idleMs < WORKING_MS;
            const state = idleMs === null ? 'running' : working ? 'working' : `idle, waiting for ${formatDuration(idleMs)}`;
            return (
              <li key={`device-${agent.pid}`} className={`grid grid-cols-[12px_minmax(5rem,12rem)_minmax(0,1fr)] items-start gap-3 py-2 ${idleMs !== null && !working ? 'opacity-70' : ''}`} data-agent-state={idleMs === null ? 'running' : working ? 'working' : 'idle'}>
                <span className={`mt-1.5 h-2.5 w-2.5 rounded-full ${working ? 'bg-good motion-safe:animate-pulse' : idleMs === null ? 'bg-good' : 'bg-muted/40'}`} role="img" aria-label={working ? 'working' : idleMs === null ? 'running' : 'idle'} />
                <div className="min-w-0">
                  <p className="truncate text-sm font-semibold text-primary">
                    {agentDisplayName(agent.tool)}{agent.model && <span className="font-normal text-secondary"> {modelDisplayName(agent.tool, agent.model)}</span>}
                  </p>
                  <p className="truncate text-[11px] text-muted">{state}</p>
                </div>
                <div className="min-w-0">
                  <p className="truncate text-sm text-secondary" title={agent.title ?? undefined}>{agent.title ?? <span className="text-muted">Untitled conversation</span>}</p>
                  <p className="truncate font-mono text-[11px] text-muted" title={agent.cwd ?? undefined}>
                    {agent.cwd ? home(agent.cwd) : 'working directory not readable'}{agent.started_at ? ` · open ${formatDuration(now - Date.parse(agent.started_at))}` : ''} · pid {agent.pid}
                  </p>
                </div>
              </li>
            );
          })}
          {activeChats.map(({ profile, session }) => (
            <li key={`chat-${session.id}`} className="grid grid-cols-[12px_minmax(5rem,12rem)_minmax(0,1fr)] items-start gap-3 py-2">
              <MessageSquare size={12} className="mt-1 text-good" aria-hidden="true" />
              <div className="min-w-0">
                <p className="truncate text-sm font-semibold text-primary">{agentDisplayName(session.backend)}{session.model && <span className="font-normal text-secondary"> {modelDisplayName(session.backend, session.model)}</span>}</p>
                <p className="truncate text-[11px] text-muted">dashboard chat</p>
              </div>
              <div className="min-w-0">
                <button type="button" onClick={() => onOpenChat(profile, session.id)} className="block max-w-full truncate text-left text-sm text-accent hover:underline">
                  {session.title ?? session.branch}
                </button>
                <p className="truncate text-xs tabular-nums text-muted">{profile} · last turn {formatDuration(now - session.lastActiveAt)} ago</p>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
