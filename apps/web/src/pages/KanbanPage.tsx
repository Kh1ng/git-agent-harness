import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Check, KanbanSquare, X } from 'lucide-react';
import type { DeviceAgentsSnapshot, ProviderKind } from '@git-agent-harness/contracts';
import { generateProviderInstanceId } from '@git-agent-harness/shared';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, ErrorState, LoadingState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { ExternalAnchor } from '../components/ExternalAnchor.js';
import { formatAge, formatDuration } from '../lib/format.js';
import { KANBAN_COLUMNS, backendLabel, buildKanban, whyNotRunning, type KanbanAgent, type KanbanBoard, type KanbanCard, type KanbanGate } from '../lib/kanbanBoard.js';

const KANBAN_REFRESH_MS = 15 * 1000;
/** Done only ever grows; the board shows the most recent few. */
const DONE_SHOWN = 8;

const CARD_EDGE: Record<KanbanCard['tone'], string> = { good: 'border-l-good', warning: 'border-l-warning', critical: 'border-l-critical', unknown: 'border-l-subtle' };
function elapsed(card: KanbanCard, now: number): string | null {
  if (!card.since) return null;
  if (card.working) return `working ${formatDuration(Math.max(0, (now - Date.parse(card.since)) / 1000))}`;
  const age = formatAge(card.since, new Date(now));
  return age && (card.column === 'done' ? `merged ${age}` : `last activity ${age}`);
}

function CardLinks({ card, onOpenWork }: { card: KanbanCard; onOpenWork: (workId: string) => void }) {
  if (!card.pullRequest?.url && !card.workId) return null;
  return (
    <div className="mt-2 flex gap-3 text-xs">
      {card.pullRequest?.url && <ExternalAnchor href={card.pullRequest.url} className="text-accent hover:underline">Pull request</ExternalAnchor>}
      {card.workId && <button type="button" onClick={() => onOpenWork(card.workId!)} className="text-accent hover:underline">History</button>}
    </div>
  );
}

/** Handing a Needs you job to an agent the person picks. */
export type KanbanAssign = {
  send: (card: KanbanCard, agent: KanbanAgent) => void;
  /** Why nothing can be assigned right now; null when it can. */
  unavailable: string | null;
};

function AssignControl({ card, agents, assign }: { card: KanbanCard; agents: KanbanAgent[]; assign: KanbanAssign }) {
  const usable = agents.filter((agent) => agent.state !== 'unavailable');
  const [agentId, setAgentId] = useState<string | null>(null);
  const [sent, setSent] = useState<string | null>(null);
  const agent = usable.find((item) => item.id === agentId) ?? usable[0];
  const blocked = card.assignHeldBy ?? assign.unavailable ?? (agent ? null : 'No agent is available right now');
  return (
    <div className="mt-2">
      {card.assignHeldBy && <p className="mb-1 text-[11px] text-muted">{card.assignHeldBy}.</p>}
      <div className="flex items-center gap-1.5">
        <select aria-label={`Agent for ${card.workId}`} value={agent?.id ?? ''} onChange={(event) => setAgentId(event.target.value)}
          className="input min-w-0 flex-1 !px-2 !py-1 text-xs">
          {agents.map((item) => (
            <option key={item.id} value={item.id} disabled={item.state === 'unavailable'}>
              {backendLabel(item.backend)}{item.state === 'unavailable' ? ' (unavailable)' : item.state === 'working' ? ' (busy)' : ''}
            </option>
          ))}
        </select>
        <button type="button" disabled={blocked !== null} title={blocked ?? `Start ${backendLabel(agent!.backend)} on ${card.workId} as a fix job`}
          onClick={() => { assign.send(card, agent!); setSent(`Sent to ${backendLabel(agent!.backend)}. The Activity feed reports what happens.`); }}
          className="btn-secondary !min-h-0 shrink-0 !px-2 !py-1 text-xs">
          Assign
        </button>
      </div>
      {sent && <p role="status" className="mt-1 text-[11px] text-good">{sent}</p>}
    </div>
  );
}

type JobCardProps = { card: KanbanCard; now: number; selected: boolean; onSelect: (key: string) => void; onOpenWork: (workId: string) => void; agents: KanbanAgent[]; assign?: KanbanAssign };

function JobCard({ card, now, selected, onSelect, onOpenWork, agents, assign }: JobCardProps) {
  const facts = [
    card.agent && (card.agentRole === 'working' ? card.agent : `built by ${card.agent}`),
    card.attempts > 0 && `attempt ${card.attempts}`,
    card.fix && card.fix.used > 0 && `fix ${card.fix.used} of ${card.fix.max}`,
    elapsed(card, now),
    card.held && 'hold on'
  ].filter((fact): fact is string => !!fact);
  return (
    <li>
      <article className={`card border-l-2 p-2.5 ${CARD_EDGE[card.tone]} ${selected ? 'ring-1 ring-accent' : ''}`}>
        <button type="button" onClick={() => onSelect(card.key)} aria-pressed={selected} className="block w-full text-left"
          title={card.working ? 'Show what is running it' : "Show why this isn't running"}>
          <span className="flex items-center gap-1.5 text-xs text-muted">
            {card.working && <span className="h-2 w-2 shrink-0 animate-pulse rounded-full bg-good" aria-label="Running now" role="img" />}
            <span className="font-mono">{card.workId ?? 'No issue'}</span>
            {card.pullRequest?.id && <span>PR {card.pullRequest.id}{card.pullRequest.draft ? ' (draft)' : ''}</span>}
            {card.managed && <StatusBadge tone="unknown" label="managed" />}
          </span>
          <span className="mt-1 line-clamp-2 block text-sm font-medium text-primary">{card.title}</span>
          <span className="mt-1 block text-xs text-secondary">{card.reason}</span>
        </button>
        {facts.length > 0 && <p className="mt-1.5 text-[11px] leading-snug text-muted">{facts.join(' · ')}</p>}
        <CardLinks card={card} onOpenWork={onOpenWork} />
        {assign && card.column === 'needs_you' && card.workId && <AssignControl card={card} agents={agents} assign={assign} />}
      </article>
    </li>
  );
}

function Gates({ gates }: { gates: KanbanGate[] }) {
  return (
    <ul aria-label="Factory-wide limits" className="mb-4 flex flex-wrap gap-2">
      {gates.map((gate) => (
        <li key={`${gate.label}-${gate.detail}`}>
          <StatusBadge tone={gate.ok === null ? 'unknown' : gate.ok ? 'good' : 'warning'} label={`${gate.label}: ${gate.detail}`} />
        </li>
      ))}
    </ul>
  );
}

function Verdict({ ok, children }: { ok: boolean | null; children: ReactNode }) {
  const Icon = ok ? Check : X;
  return (
    <li className="flex items-start gap-2 text-xs text-secondary">
      <Icon size={13} className={`mt-0.5 shrink-0 ${ok ? 'text-good' : ok === null ? 'text-muted' : 'text-warning'}`} aria-label={ok ? 'Fine' : ok === null ? 'Unknown' : 'In the way'} />
      <span className="min-w-0 break-words">{children}</span>
    </li>
  );
}

function WhyPanel({ board, card, now, onClose, onOpenWork, assign }: { board: KanbanBoard; card: KanbanCard; now: number; onClose: () => void; onOpenWork: (workId: string) => void; assign?: KanbanAssign }) {
  const name = card.workId ?? card.title;
  const settled = card.working || card.column === 'done';
  const heading = card.working ? `${name} is running` : card.column === 'done' ? `${name} is done` : `Why isn't ${name} running?`;
  return (
    <section aria-label={heading}>
      <button type="button" onClick={onClose} className="mb-2 inline-flex items-center gap-1 text-xs text-secondary hover:text-primary">
        <X size={13} aria-hidden="true" />
        Close
      </button>
      <div className="card p-3">
        <h3 className="text-sm font-semibold text-primary">{heading}</h3>
        <p className="mt-0.5 text-xs text-muted">{card.title}</p>
        <p className="mt-2 text-xs text-secondary">{card.reason}</p>
        {card.working && <p className="mt-1 text-xs text-secondary">{[card.agent ?? 'The agent process is not visible yet', elapsed(card, now)].filter(Boolean).join(' · ')}</p>}
        <CardLinks card={card} onOpenWork={onOpenWork} />

        {card.blocks.length > 0 && (
          <>
            <h4 className="mt-3 text-[11px] font-semibold uppercase tracking-wide text-muted">This job</h4>
            <ul className="mt-1 space-y-1">{card.blocks.map((block) => <Verdict key={block} ok={false}>{block}</Verdict>)}</ul>
          </>
        )}
        {!settled && (
          <>
            <h4 className="mt-3 text-[11px] font-semibold uppercase tracking-wide text-muted">Factory-wide</h4>
            <ul className="mt-1 space-y-1">{board.gates.map((gate) => <Verdict key={`${gate.label}-${gate.detail}`} ok={gate.ok}>{gate.label}: {gate.detail}</Verdict>)}</ul>
            <h4 className="mt-3 text-[11px] font-semibold uppercase tracking-wide text-muted">Each agent</h4>
            {card.column === 'needs_you'
              ? (
                <>
                  <p className="mt-1 text-xs text-secondary">No agent will pick this one up on its own: it waits for you.{assign && card.workId && ' You can hand it to an agent:'}</p>
                  {assign && card.workId && <AssignControl key={card.key} card={card} agents={board.agents} assign={assign} />}
                </>
              )
              : card.job === 'merge'
                ? <p className="mt-1 text-xs text-secondary">The controller handles this step; it does not need a coding agent.</p>
                : <ul className="mt-1 space-y-1">{whyNotRunning(board, card).map(({ agent, ok, verdict }) => <Verdict key={agent.id} ok={ok}><span className="text-primary">{agent.name}</span>: {verdict}</Verdict>)}</ul>}
          </>
        )}
      </div>
    </section>
  );
}

/** The board, and beside it the why-not answer for a selected card, from an already derived board: no fetching here. */
export function KanbanView({ board, now, onOpenWork, assign }: { board: KanbanBoard; now: number; onOpenWork: (workId: string) => void; assign?: KanbanAssign }) {
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const side = useRef<HTMLElement>(null);
  const selected = board.cards.find((card) => card.key === selectedKey) ?? null;
  const select = (key: string) => setSelectedKey(key === selectedKey ? null : key);
  // Below the board on a narrow screen: bring the answer into view.
  useEffect(() => { if (selectedKey) side.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }); }, [selectedKey]);
  return (
    <>
      <Gates gates={board.gates} />
      <div className={selected ? 'grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_20rem]' : ''}>
        <div className="flex items-start gap-3 overflow-x-auto pb-2" role="group" aria-label="Job board">
          {KANBAN_COLUMNS.map((column) => {
            const cards = board.cards.filter((card) => card.column === column.key);
            const shown = column.key === 'done' ? cards.slice(0, DONE_SHOWN) : cards;
            return (
              <section key={column.key} aria-label={`${column.label}, ${cards.length} ${cards.length === 1 ? 'job' : 'jobs'}`}
                className={`shrink-0 rounded-lg border border-subtle bg-card/40 ${cards.length ? 'w-60' : 'w-28'}`}>
                <h3 className="flex items-baseline justify-between gap-2 border-b border-subtle px-2.5 py-2 text-xs font-semibold text-primary" title={column.hint}>
                  {column.label}
                  <span className="font-normal text-muted">{cards.length}</span>
                </h3>
                {cards.length === 0
                  ? <p className="px-2.5 py-3 text-[11px] text-muted">Nothing here</p>
                  : (
                    <ul className="space-y-2 p-2">
                      {shown.map((card) => <JobCard key={card.key} card={card} now={now} selected={card.key === selectedKey} onSelect={select} onOpenWork={onOpenWork} agents={board.agents} assign={assign} />)}
                      {cards.length > shown.length && <li className="px-1 text-[11px] text-muted">and {cards.length - shown.length} older</li>}
                    </ul>
                  )}
              </section>
            );
          })}
        </div>
        {selected && (
          <aside ref={side} className="min-w-0 xl:sticky xl:top-0">
            <WhyPanel board={board} card={selected} now={now} onClose={() => setSelectedKey(null)} onOpenWork={onOpenWork} assign={assign} />
          </aside>
        )}
      </div>
      {board.notPickedUp.length > 0 && (
        <details className="card mt-4 p-3">
          <summary className="cursor-pointer text-xs font-semibold text-primary">Not picked up ({board.notPickedUp.length})</summary>
          <ul className="mt-2 space-y-1.5">
            {board.notPickedUp.map((item) => (
              <li key={`${item.workId}-${item.title}`} className="text-xs text-secondary">
                <span className="font-mono text-muted">{item.workId}</span> {item.title}
                {item.managed && <span className="ml-1.5 inline-flex align-middle"><StatusBadge tone="unknown" label="managed" /></span>}
                <span className="block text-[11px] text-muted">{item.reason}</span>
              </li>
            ))}
          </ul>
        </details>
      )}
    </>
  );
}

type KanbanPageProps = {
  /** The device's agent processes, polled by the app shell. */
  deviceAgents: { data: DeviceAgentsSnapshot | null; error: string | null };
  onOpenWork: (workId: string) => void;
};

/** One page for "what is running, what is not, and why": a job board whose cards each explain themselves. */
export function KanbanPage({ deviceAgents, onOpenWork }: KanbanPageProps) {
  const { profile: wsProfile, controllerActivity, sendMessage, isConnected } = useWebSocket();
  const profileOverride = useUiStore((state) => state.profileOverride);
  const profile = profileOverride ?? wsProfile ?? undefined;
  const status = useGahStore((state) => state.status);
  const quota = useGahStore((state) => state.quota);
  const loopStatus = useGahStore((state) => state.loopStatus);
  const fetchStatus = useGahStore((state) => state.fetchStatus);
  const fetchQuota = useGahStore((state) => state.fetchQuota);
  const fetchLoopStatus = useGahStore((state) => state.fetchLoopStatus);
  const profiles = useGahStore((state) => state.profiles);
  const fetchProfiles = useGahStore((state) => state.fetchProfiles);
  const [now, setNow] = useState(Date.now);

  const refresh = (force = true) => {
    setNow(Date.now());
    void fetchStatus(profile, { force });
    // The app shell keeps this same quota snapshot fresh for the navbar.
    void fetchQuota({ profile, since: '7d' });
    if (profile) void fetchLoopStatus(profile, { force });
  };
  useEffect(() => { refresh(false); void fetchProfiles(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [profile]);
  useAutoRefresh(refresh, KANBAN_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const snapshot = status.key === (profile ?? '') ? status.data : null;
  const board = useMemo(() => buildKanban({
    status: snapshot,
    quota: quota.data,
    controllerRuns: controllerActivity.filter((run) => !profile || !run.profile || run.profile === profile),
    factoryAgents: deviceAgents.data?.factory_agents ?? [],
    loopRunning: loopStatus.data?.running ?? null,
    now
  }), [snapshot, quota.data, controllerActivity, deviceAgents.data, loopStatus.data, profile, now]);

  const repo = profiles.data?.find((item) => item.name === profile)?.repo ?? null;
  // The same socket message the Factory page's dispatch form and the work drawer's Re-dispatch send.
  const assign: KanbanAssign = {
    unavailable: !isConnected ? 'Disconnected from the server' : !profile || !repo ? 'Repository details are not available yet' : null,
    send: (card, agent) => {
      if (!profile || !repo || !card.workId) return;
      const backend = agent.backend as ProviderKind;
      sendMessage({ type: 'session.start', requestId: `kanban_assign_${Date.now()}`, profile, providerKind: backend, instanceId: generateProviderInstanceId(backend, 0), repo, mode: 'fix', backend, target: card.workId });
      window.setTimeout(() => refresh(), 3000);
    }
  };

  return (
    <div>
      <PageHeader title="Kanban" description="What is running, what is not, and why." onRefresh={() => refresh()} refreshing={status.loading} lastUpdated={status.fetchedAt} />
      {!snapshot && status.error ? <ErrorState message={status.error} endpoint="/api/status" onRetry={() => refresh()} />
        : !snapshot ? <LoadingState label="Loading the board…" />
          : board.cards.length === 0 ? <EmptyState icon={KanbanSquare} title="No jobs yet" description="Issues the factory accepts show up here as soon as it sees them." />
            : <KanbanView board={board} now={now} onOpenWork={onOpenWork} assign={assign} />}
    </div>
  );
}
