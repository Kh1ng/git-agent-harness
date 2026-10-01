import { useEffect, useState } from 'react';
import { ExternalLink, MessageSquare, Orbit, Sparkles } from 'lucide-react';
import type { PlanningEpic, PlanningMap, PlanningNode, PlanningSettings } from '@git-agent-harness/contracts';
import { DEFAULT_PLANNING_SETTINGS } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { ExternalAnchor } from '../components/ExternalAnchor.js';
import { STATE_LABEL, StarMap } from '../components/StarMap.js';
import { EmptyState, ErrorState, LoadingState } from '../components/ui/EmptyState.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { useChatProfiles } from '../hooks/useChatProfiles.js';
import { readNavigation, updateNavigation, type Page } from '../lib/navigationState.js';
import { useUiStore } from '../store/uiStore.js';
import { useWebSocket } from '../ws/WebSocketContext.js';

const BADGE: Record<PlanningNode['state'], string> = {
  ready: 'badge badge-good',
  blocked: 'badge badge-warning',
  parent: 'badge badge-unknown',
  done: 'badge badge-unknown'
};

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Mirrors the server: a repository-relative `.md` path that cannot leave the checkout. */
const validAnswersPath = (path: string) => path.length > 0 && path.length <= 200 && path.endsWith('.md')
  && !path.startsWith('/') && !path.includes('\\') && !/[\x00-\x1f\x7f]/.test(path)
  && path.split('/').every((part) => part.length > 0 && part !== '.' && part !== '..');

/**
 * Planning (#1241): an epic's issues as a star map, the work that can start
 * now, and grill-me chats that turn an idea into decisions and tickets.
 * Read-only: the map never files or edits issues.
 */
export function PlanningPage({ onNavigate }: { onNavigate: (page: Page) => void }) {
  const { isConnected, reconnectSeq, profile: wsProfile } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
  const setProfileOverride = useUiStore((s) => s.setProfileOverride);
  const [profiles] = useChatProfiles(reconnectSeq);
  const local = profiles.filter((candidate) => !candidate.remote);
  const profile = profileOverride ?? wsProfile ?? 'gah';
  const projectIsLocal = local.some((candidate) => candidate.name === profile);

  const [epics, setEpics] = useState<PlanningEpic[] | null>(null);
  const [epicsError, setEpicsError] = useState<string | null>(null);
  const [epic, setEpic] = useState<number | null>(() => {
    const nav = readNavigation();
    return nav.epic && nav.profile === profile ? Number(nav.epic) : null;
  });
  const [map, setMap] = useState<PlanningMap | null>(null);
  const [mapError, setMapError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const [epoch, setEpoch] = useState(0);
  const [lastUpdated, setLastUpdated] = useState<number | null | undefined>(undefined);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [grillOpen, setGrillOpen] = useState(false);

  useEffect(() => { updateNavigation({ profile, epic: epic === null ? null : String(epic) }); }, [profile, epic]);

  useEffect(() => {
    let cancelled = false;
    setEpics(null);
    setEpicsError(null);
    if (!projectIsLocal) return;
    gahApi.getPlanningEpics(profile, epoch > 0)
      .then(({ epics }) => {
        if (cancelled) return;
        setEpics(epics);
        setLastUpdated(Date.now());
        setEpic((current) => current ?? epics.find((candidate) => candidate.open)?.number ?? null);
      })
      .catch((error) => { if (!cancelled) setEpicsError(errorText(error)); });
    return () => { cancelled = true; };
  }, [profile, projectIsLocal, epoch, reconnectSeq]);

  useEffect(() => {
    let cancelled = false;
    setMap(null);
    setMapError(null);
    setSelected(null);
    if (epic === null || !projectIsLocal) return;
    setLoading(true);
    gahApi.getPlanningMap(profile, epic, epoch > 0)
      .then((loaded) => { if (!cancelled) { setMap(loaded); setLastUpdated(Date.now()); } })
      .catch((error) => { if (!cancelled) setMapError(errorText(error)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [profile, epic, projectIsLocal, epoch, reconnectSeq]);

  const openChat = async (request: Parameters<typeof gahApi.startPlanningChat>[0]) => {
    setStarting(true);
    setStartError(null);
    try {
      const session = await gahApi.startPlanningChat(request);
      setProfileOverride(profile);
      updateNavigation({ profile, chat: session.id, epic: null });
      onNavigate('chat');
    } catch (error) {
      setStartError(errorText(error));
    } finally {
      setStarting(false);
    }
  };

  const node = map?.nodes.find((candidate) => candidate.number === selected) ?? null;

  return (
    <div className="flex min-h-0 flex-col">
      <PageHeader
        title="Planning"
        description="An epic's issues as a map: what is done, what waits on what, and what can start now. Read-only."
        lastUpdated={lastUpdated}
        onRefresh={projectIsLocal ? () => setEpoch((value) => value + 1) : undefined}
        refreshing={loading}
        actions={
          <button type="button" className="btn-primary" disabled={!isConnected || !projectIsLocal}
            onClick={() => setGrillOpen((open) => !open)} aria-expanded={grillOpen}>
            <Sparkles size={15} aria-hidden="true" /> Grill-me
          </button>
        }
      />

      <div className="mb-4 flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-sm text-secondary">
          Project
          <select className="input max-w-[16rem]" value={projectIsLocal ? profile : ''}
            onChange={(event) => { setEpic(null); setProfileOverride(event.target.value); }}>
            {!projectIsLocal && <option value="" disabled>Choose a project</option>}
            {local.map((candidate) => (
              <option key={candidate.name} value={candidate.name}>{candidate.display_name || candidate.name}</option>
            ))}
          </select>
        </label>
        {epics && epics.length > 0 && (
          <label className="flex items-center gap-2 text-sm text-secondary">
            Epic
            <select className="input max-w-[22rem]" value={epic ?? ''}
              onChange={(event) => setEpic(event.target.value ? Number(event.target.value) : null)}>
              {epic === null && <option value="">Choose an epic</option>}
              {epics.map((candidate) => (
                <option key={candidate.number} value={candidate.number}>
                  #{candidate.number} {candidate.title}{candidate.open ? '' : ' (closed)'}: {candidate.open_children} of {candidate.children} open
                </option>
              ))}
            </select>
          </label>
        )}
      </div>

      {grillOpen && projectIsLocal && (
        <GrillPanel profile={profile} epic={map ? { number: map.epic, title: map.nodes.find((n) => n.number === map.epic)?.title ?? '' } : null}
          starting={starting} onStart={(idea, useEpic) => void openChat({ profile, kind: 'grill', idea, ...(useEpic && epic !== null ? { epic } : {}) })} />
      )}
      {startError && <p role="alert" className="mb-3 text-sm text-critical">{startError}</p>}

      {!projectIsLocal ? (
        <EmptyState icon={Orbit} title="Choose a project configured on this node"
          description="Planning reads issues through this node's own project settings. Projects that live on a worker are not mapped here." />
      ) : epicsError ? (
        <ErrorState message={epicsError} onRetry={() => setEpoch((value) => value + 1)} />
      ) : epics === null ? (
        <LoadingState label="Reading issues…" />
      ) : epics.length === 0 ? (
        <EmptyState icon={Orbit} title="No epics yet"
          description="An epic is an issue with sub-issues, an issue other issues name with a `Parent: #N` line, or one labelled `epic`. Use Grill-me to plan one." />
      ) : mapError ? (
        <ErrorState message={mapError} onRetry={() => setEpoch((value) => value + 1)} />
      ) : !map ? (
        epic === null ? null : <LoadingState label="Mapping the epic…" />
      ) : (
        <div className="grid min-w-0 items-start gap-4 md:grid-cols-[minmax(0,1fr)_20rem]">
          <section className="card hidden p-2 md:block" aria-label="Star map">
            <StarMap map={map} selected={selected} onSelect={(picked) => setSelected(picked.number)} />
            <Legend />
          </section>
          <aside className="min-w-0 space-y-4">
            {node && (
              <NodeDetail node={node} map={map} starting={starting} onSelect={setSelected}
                onDiscuss={() => void openChat({ profile, kind: 'ticket', epic: map.epic, ticket: node.number })} />
            )}
            <FrontierList map={map} selected={selected} onSelect={setSelected} />
          </aside>
        </div>
      )}
    </div>
  );
}

function Legend() {
  const items: [string, string][] = [['text-accent', 'Ready'], ['text-warning', 'Blocked'], ['text-secondary', 'In progress below'], ['text-muted', 'Done']];
  return (
    <ul className="flex flex-wrap gap-x-4 gap-y-1 px-3 pb-2 text-xs text-muted" aria-label="Legend">
      {items.map(([color, label]) => (
        <li key={label} className="flex items-center gap-1.5">
          <span className={`inline-block h-2.5 w-2.5 rounded-full bg-current ${color}`} aria-hidden="true" />{label}
        </li>
      ))}
      <li className="flex items-center gap-1.5"><span className="text-warning" aria-hidden="true">→</span>blocks</li>
      <li className="flex items-center gap-1.5"><span className="inline-block h-2.5 w-2.5 rounded-full border border-dashed border-current" aria-hidden="true" />outside the epic</li>
    </ul>
  );
}

function IssueLink({ number, map, onSelect }: { number: number; map: PlanningMap; onSelect: (number: number) => void }) {
  const known = map.nodes.some((candidate) => candidate.number === number);
  return known
    ? <button type="button" className="text-accent hover:underline" onClick={() => onSelect(number)}>#{number}</button>
    : <span className="text-muted">#{number}</span>;
}

/** The work that can start now; on a phone this is the whole page. */
function FrontierList({ map, selected, onSelect }: { map: PlanningMap; selected: number | null; onSelect: (number: number) => void }) {
  const frontier = map.frontier.map((number) => map.nodes.find((node) => node.number === number)).filter((node): node is PlanningNode => !!node);
  const waiting = map.nodes.filter((node) => node.state === 'blocked' && node.depth !== null);
  const done = map.nodes.filter((node) => node.state === 'done' && node.depth !== null).length;
  const total = map.nodes.filter((node) => node.depth !== null && node.number !== map.epic).length;
  return (
    <section className="card-padded space-y-3" aria-label="Frontier">
      <div>
        <h3 className="text-sm font-semibold text-primary">Can start now</h3>
        <p className="text-xs text-muted">{done} of {total} done under #{map.epic}.</p>
      </div>
      {frontier.length === 0 ? (
        <p className="text-sm text-secondary">Nothing is ready: every open issue is blocked or waits on its own sub-issues.</p>
      ) : (
        <ul className="space-y-1.5">
          {frontier.map((node) => (
            <li key={node.number}>
              <button type="button" onClick={() => onSelect(node.number)} aria-current={node.number === selected ? 'true' : undefined}
                className={`w-full rounded-md px-2 py-1.5 text-left text-sm hover:bg-white/5 ${node.number === selected ? 'bg-white/5' : ''}`}>
                <span className="font-medium text-accent">#{node.number}</span> <span className="text-primary">{node.title}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {waiting.length > 0 && (
        <details>
          <summary className="cursor-pointer text-xs font-medium uppercase tracking-wide text-muted">Waiting ({waiting.length})</summary>
          <ul className="mt-2 space-y-1.5 text-sm">
            {waiting.map((node) => (
              <li key={node.number}>
                <button type="button" onClick={() => onSelect(node.number)} className="text-left hover:underline">
                  <span className="text-warning">#{node.number}</span> <span className="text-primary">{node.title}</span>
                </button>
                <span className="block pl-1 text-xs text-muted">waits on {node.waiting_on.map((n) => `#${n}`).join(', ')}</span>
              </li>
            ))}
          </ul>
        </details>
      )}
      {map.missing.length > 0 && (
        <p className="text-xs text-muted">Not found in this repository: {map.missing.map((n) => `#${n}`).join(', ')}.</p>
      )}
    </section>
  );
}

function NodeDetail({ node, map, starting, onSelect, onDiscuss }: {
  node: PlanningNode;
  map: PlanningMap;
  starting: boolean;
  onSelect: (number: number) => void;
  onDiscuss: () => void;
}) {
  const blocks = map.edges.filter((edge) => edge.kind === 'blocks' && edge.from === node.number).map((edge) => edge.to);
  const blockedBy = map.edges.filter((edge) => edge.kind === 'blocks' && edge.to === node.number).map((edge) => edge.from);
  return (
    <section className="card-padded space-y-3" aria-label={`Issue #${node.number}`}>
      <div className="flex items-start justify-between gap-2">
        <h3 className="min-w-0 text-sm font-semibold text-primary">#{node.number} {node.title}</h3>
        <span className={BADGE[node.state]}>{STATE_LABEL[node.state]}</span>
      </div>
      {node.depth === null && <p className="text-xs text-muted">Outside this epic; issues in it wait on this one.</p>}
      {blockedBy.length > 0 && (
        <p className="text-xs text-secondary">Blocked by {blockedBy.map((number, index) => (
          <span key={number}>{index > 0 && ', '}<IssueLink number={number} map={map} onSelect={onSelect} /></span>
        ))}</p>
      )}
      {blocks.length > 0 && (
        <p className="text-xs text-secondary">Blocks {blocks.map((number, index) => (
          <span key={number}>{index > 0 && ', '}<IssueLink number={number} map={map} onSelect={onSelect} /></span>
        ))}</p>
      )}
      <div className="flex flex-wrap gap-2">
        {node.url && (
          <ExternalAnchor href={node.url} className="btn-secondary text-xs">
            <ExternalLink size={13} aria-hidden="true" /> Open issue
          </ExternalAnchor>
        )}
        {node.number !== map.epic && node.depth !== null && (
          <button type="button" className="btn-secondary text-xs" disabled={starting} onClick={onDiscuss}>
            <MessageSquare size={13} aria-hidden="true" /> Discuss in chat
          </button>
        )}
      </div>
    </section>
  );
}

/** Starts a grill-me chat and keeps where its answers go. */
function GrillPanel({ profile, epic, starting, onStart }: {
  profile: string;
  epic: { number: number; title: string } | null;
  starting: boolean;
  onStart: (idea: string, useEpic: boolean) => void;
}) {
  const [idea, setIdea] = useState('');
  const [useEpic, setUseEpic] = useState(true);
  const [saved, setSaved] = useState<PlanningSettings | null>(null);
  const [draft, setDraft] = useState<PlanningSettings>(DEFAULT_PLANNING_SETTINGS);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    gahApi.getPlanningSettings(profile)
      .then((settings) => { if (!cancelled) { setSaved(settings); setDraft(settings); } })
      .catch((caught) => { if (!cancelled) setError(errorText(caught)); });
    return () => { cancelled = true; };
  }, [profile]);

  const pathValid = validAnswersPath(draft.path);
  const changed = saved !== null && (saved.answers !== draft.answers || saved.path !== draft.path);

  const start = async () => {
    setError(null);
    try {
      if (changed) setSaved(await gahApi.setPlanningSettings(profile, draft));
      onStart(idea, useEpic);
    } catch (caught) {
      setError(errorText(caught));
    }
  };

  return (
    <section className="card-padded mb-4 space-y-3" aria-label="Grill-me">
      <div>
        <h3 className="text-sm font-semibold text-primary">Grill-me</h3>
        <p className="text-xs text-muted">A chat asks one question at a time until the plan is clear, then proposes decisions and tickets. Nothing is recorded until you confirm in the chat.</p>
      </div>
      <label className="block text-sm text-secondary">
        What do you want to plan?
        <textarea className="input mt-1 block min-h-[5rem] w-full" maxLength={4000} value={idea}
          onChange={(event) => setIdea(event.target.value)} placeholder="Optional. Leave empty and the chat asks." />
      </label>
      {epic && (
        <label className="flex items-center gap-2 text-sm text-secondary">
          <input type="checkbox" checked={useEpic} onChange={(event) => setUseEpic(event.target.checked)} />
          Plan within #{epic.number} {epic.title}
        </label>
      )}
      <fieldset className="space-y-1.5 text-sm text-secondary">
        <legend className="mb-1 text-xs font-medium uppercase tracking-wide text-muted">Record answers in</legend>
        <label className="flex items-center gap-2">
          <input type="radio" name="planning-answers" checked={draft.answers === 'issues'} onChange={() => setDraft({ ...draft, answers: 'issues' })} />
          Issues on GitHub or GitLab
        </label>
        <label className="flex flex-wrap items-center gap-2">
          <input type="radio" name="planning-answers" checked={draft.answers === 'file'} onChange={() => setDraft({ ...draft, answers: 'file' })} />
          A Markdown file in the repository:
          <input className="input w-56" value={draft.path} aria-label="Answers file path" aria-invalid={!pathValid}
            disabled={draft.answers !== 'file'} onChange={(event) => setDraft({ ...draft, path: event.target.value })} />
        </label>
        {!pathValid && draft.answers === 'file' && <p className="text-xs text-critical">Use a path inside the repository that ends in .md.</p>}
      </fieldset>
      {error && <p role="alert" className="text-sm text-critical">{error}</p>}
      <button type="button" className="btn-primary" disabled={starting || saved === null || (draft.answers === 'file' && !pathValid)} onClick={() => void start()}>
        <Sparkles size={15} aria-hidden="true" /> {starting ? 'Starting…' : 'Start grill-me'}
      </button>
    </section>
  );
}
