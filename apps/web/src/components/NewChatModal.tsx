import { useEffect, useMemo, useRef, useState } from 'react';
import { FolderGit2, Cpu, X, CircleDot, GitPullRequest, MessageSquare, Star } from 'lucide-react';
import type { BackendInstanceSummary, ChatIssueSummary, ChatPrSummary, ChatSessionProjectGroup, ManagerModelInfo, ProfileSummary, ProjectSummary } from '@git-agent-harness/contracts';
import { ChatNodePicker } from './ChatNodePicker.js';
import { useChatNodes } from '../hooks/useChatNodes.js';
import { BoundedCollection } from './BoundedCollection.js';
import { backendInstancesApi, gahApi } from '../api/client.js';
import type { ManagerBackendInfo } from '@git-agent-harness/contracts';

export type ChatProfile = ProfileSummary & Partial<Pick<ProjectSummary, 'node_id' | 'chat_profile'>> & { remote?: boolean; catalogName?: string };
export type ChatSource = 'blank' | 'issue' | 'pr';

const PINNED_PROJECTS_KEY = 'gah.chat.pinned-projects';

function loadPinnedProjects(): string[] {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(PINNED_PROJECTS_KEY) ?? '[]');
    return Array.isArray(value) ? value.filter((name): name is string => typeof name === 'string') : [];
  } catch {
    return [];
  }
}

function creationError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/permission|forbidden|not authorized/i.test(message)) return `Permission denied. ${message}`;
  if (/branch.*(?:conflict|exists)|(?:conflict|exists).*branch/i.test(message)) return `Branch conflict. ${message}`;
  return message;
}

interface NewChatModalProps {
  open: boolean;
  currentProfile: string;
  profiles: ChatProfile[];
  nodesRefreshKey?: string;
  backends: ManagerBackendInfo[];
  launcher?: boolean;
  onClose: () => void;
  onViewAllProjects?: () => void;
  /** Blank chat: (profile, sessionId). Issue/PR chat: same shape — the
   * session is opened the same way either way. */
  onCreated: (profile: string, sessionId: string) => void;
}

/**
 * The T3-style new-chat flow: choose a project, choose a node, choose a
 * provider/model. The created conversation is bound to a fresh worktree —
 * each node retains its own checkout when the conversation moves. The branch
 * survives archive/reclaim.
 *
 * "From issue" grabs a provider issue instead: the session branches for the
 * issue (`gah/issue/<repo>-<n>`), the issue is marked in progress, and the
 * conversation opens seeded with the issue body.
 *
 * "From PR" opens a read-only chat seeded with a pull request — no branch,
 * no worktree, nothing at the provider is touched.
 */
export function NewChatModal({ open, currentProfile, profiles, backends, launcher = false, onClose, onCreated, onViewAllProjects, nodesRefreshKey = '' }: NewChatModalProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [project, setProject] = useState(currentProfile);
  const [nodeChoice, setNodeChoice] = useState<{ project: string; nodeId: string } | null>(null);
  const [backend, setBackend] = useState<string>('');
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [model, setModel] = useState<string | null>(null);
  const [backendInstances, setBackendInstances] = useState<BackendInstanceSummary[]>([]);
  const [backendInstance, setBackendInstance] = useState<string | null>(null);
  const [title, setTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<ChatSource>('blank');
  const [stage, setStage] = useState<'launcher' | 'create'>('create');
  const [projectGroups, setProjectGroups] = useState<ChatSessionProjectGroup[]>([]);
  const [launcherLoading, setLauncherLoading] = useState(false);
  const [launcherError, setLauncherError] = useState<string | null>(null);
  const [launcherRetry, setLauncherRetry] = useState(0);
  const [pinnedProjects, setPinnedProjects] = useState(loadPinnedProjects);
  const [issues, setIssues] = useState<ChatIssueSummary[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(false);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const [issue, setIssue] = useState<ChatIssueSummary | null>(null);
  const [prs, setPrs] = useState<ChatPrSummary[]>([]);
  const [prsLoading, setPrsLoading] = useState(false);
  const [prsError, setPrsError] = useState<string | null>(null);
  const [sourceRetry, setSourceRetry] = useState(0);
  const [pr, setPr] = useState<ChatPrSummary | null>(null);

  const nodeSnapshot = useChatNodes(project, backend || null, open, nodesRefreshKey);
  const projectInfo = profiles.find(candidate => candidate.name === project);
  const remoteProject = projectInfo?.remote ?? false;
  const nodeId = mode !== 'blank' ? nodeSnapshot.nodes.find(node => node.role === 'central')?.nodeId ?? ''
    : nodeChoice?.project === project ? nodeChoice.nodeId
    : projectInfo?.node_id ?? nodeSnapshot.nodes.find(node => node.role === 'central')?.nodeId ?? '';
  const selectedNode = nodeSnapshot.nodes.find(node => node.nodeId === nodeId);
  const nodeReady = !nodeSnapshot.loading && !nodeSnapshot.error && !!(selectedNode?.eligible ?? selectedNode?.chatCapable);
  const implementedBackends = backends.filter((b) => b.implemented);

  useEffect(() => {
    if (open) dialog.current?.showModal();
    else dialog.current?.close();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    setProject(currentProfile);
    setError(null);
    setTitle('');
    setQuery('');
    setModel(null);
    setBackendInstance(null);
    setMode('blank');
    setStage(launcher ? 'launcher' : 'create');
    setNodeChoice(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, launcher]);

  useEffect(() => {
    if (!open || stage !== 'launcher') return;
    let cancelled = false;
    setLauncherLoading(true);
    setLauncherError(null);
    gahApi.getAllChatSessions()
      .then(({ projects }) => { if (!cancelled) setProjectGroups(projects); })
      .catch((err) => { if (!cancelled) setLauncherError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (!cancelled) setLauncherLoading(false); });
    return () => { cancelled = true; };
  }, [open, stage, launcherRetry]);

  // A different source discards its selection; retrying the same source does not.
  useEffect(() => {
    setNodeChoice(null);
    setIssues([]);
    setIssue(null);
    setPrs([]);
    setPr(null);
  }, [open, mode, project]);

  // Issue list follows the selected project in issue mode.
  useEffect(() => {
    if (!open || mode !== 'issue') return;
    let cancelled = false;
    setIssuesError(null);
    setIssuesLoading(true);
    gahApi
      .getChatIssues(project)
      .then(({ issues }) => { if (!cancelled) setIssues(issues); })
      .catch((err) => { if (!cancelled) setIssuesError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (!cancelled) setIssuesLoading(false); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode, project, sourceRetry]);

  // PR list follows the selected project in PR mode.
  useEffect(() => {
    if (!open || mode !== 'pr') return;
    let cancelled = false;
    setPrsError(null);
    setPrsLoading(true);
    gahApi
      .getChatPrs(project)
      .then(({ prs }) => { if (!cancelled) setPrs(prs); })
      .catch((err) => { if (!cancelled) setPrsError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (!cancelled) setPrsLoading(false); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode, project, sourceRetry]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // Default to the profile's configured backend, else the first implemented one.
    gahApi
      .getManagerChatSettings()
      .then((settings) => {
        if (cancelled) return;
        const preferred = settings.profileOverrides[project] ?? settings.defaultBackend;
        setBackend(implementedBackends.some((b) => b.id === preferred) ? preferred : implementedBackends[0]?.id ?? '');
      })
      .catch(() => { if (!cancelled) setBackend(implementedBackends[0]?.id ?? ''); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, project]);

  useEffect(() => {
    if (!open || !project || !backend) return;
    let cancelled = false;
    backendInstancesApi.list(project)
      .then(({ backend_instances: instances }) => {
        if (cancelled) return;
        const eligible = instances.filter((instance) => instance.enabled && instance.executable_resolved !== false && instance.auth_ready !== false && instance.logical_backend === backend);
        setBackendInstances(eligible);
        setBackendInstance((selected) => eligible.some((instance) => instance.backend_instance === selected) ? selected : null);
      })
      .catch(() => { if (!cancelled) { setBackendInstances([]); setBackendInstance(null); } });
    return () => { cancelled = true; };
  }, [open, project, backend]);

  useEffect(() => {
    if (!open || !backend || !nodeId || !nodeReady) {
      setModels([]);
      setModel(null);
      return;
    }
    let cancelled = false;
    setModels([]);
    setModel(null);
    gahApi
      .getManagerChatModelsForBackend(project, backend, nodeId || undefined, backendInstance)
      .then(({ models, currentModelId }) => {
        if (cancelled) return;
        setModels(models);
        setModel(models.length > 0 ? currentModelId ?? models[0].id : null);
      })
      .catch(() => {
        if (!cancelled) {
          setModels([]);
          setModel(null);
        }
      });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, project, backend, backendInstance, nodeId, nodeReady]);
  const launcherProjects = useMemo(() => {
    const lastActive = new Map(projectGroups.map((group) => [
      group.profile,
      Math.max(0, ...group.sessions.map((session) => session.lastActiveAt))
    ]));
    return [...profiles]
      .sort((a, b) => Number(pinnedProjects.includes(b.name)) - Number(pinnedProjects.includes(a.name))
        || (lastActive.get(b.name) ?? 0) - (lastActive.get(a.name) ?? 0)
        || (a.display_name || a.name).localeCompare(b.display_name || b.name))
      .slice(0, 6);
  }, [profiles, projectGroups, pinnedProjects]);
  if (!open) return null;

  const chooseProject = (profile: string, source: ChatSource) => {
    setProject(profile);
    setMode(source);
    setStage('create');
  };

  const togglePinnedProject = (profile: string) => {
    setPinnedProjects((current) => {
      const next = current.includes(profile) ? current.filter((name) => name !== profile) : [...current, profile];
      try { window.localStorage.setItem(PINNED_PROJECTS_KEY, JSON.stringify(next)); } catch { /* Persistence is optional. */ }
      return next;
    });
  };

  const startFromSource = async (source: Exclude<ChatSource, 'blank'>, number: number) => {
    if (!backend || remoteProject || creating) return;
    setCreating(true);
    setError(null);
    try {
      const { session } = source === 'issue'
        ? await gahApi.startChatFromIssue(project, number, backend, model)
        : await gahApi.startChatFromPr(project, number, backend, model);
      onCreated(project, session.id);
      dialog.current?.close();
    } catch (err) {
      setError(creationError(err));
    } finally {
      setCreating(false);
    }
  };

  const create = async () => {
    if (!backend || (mode === 'blank' && !nodeReady) || (mode !== 'blank' && remoteProject)) return;
    setCreating(true);
    setError(null);
    try {
      if (mode === 'issue') {
        if (!issue) return;
        const { session } = await gahApi.startChatFromIssue(project, issue.number, backend, model);
        onCreated(project, session.id);
      } else if (mode === 'pr') {
        if (!pr) return;
        const { session } = await gahApi.startChatFromPr(project, pr.number, backend, model);
        onCreated(project, session.id);
      } else {
        const session = await gahApi.createChatSession(project, backend, model, title.trim() || undefined, nodeId, backendInstance);
        onCreated(project, session.id);
      }
      dialog.current?.close();
    } catch (err) {
      setError(creationError(err));
    } finally {
      setCreating(false);
    }
  };

  return (
    <dialog ref={dialog} onClose={onClose} aria-label="New chat"
      onClick={(event) => { if (event.target === event.currentTarget) dialog.current?.close(); }}
      className="m-auto w-[calc(100%_-_2rem)] max-w-lg max-h-[85vh] overflow-visible border-0 bg-transparent p-0 text-primary backdrop:bg-black/60">
      <div className="card w-full max-h-[85vh] overflow-y-auto p-5 space-y-5">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-primary">{stage === 'launcher' ? 'Start a chat' : 'New chat'}</h2>
          <button onClick={() => dialog.current?.close()} className="rounded p-1 text-muted hover:bg-white/5 hover:text-primary" aria-label="Close">
            <X size={16} aria-hidden="true" />
          </button>
        </div>

        {stage === 'launcher' ? (
          <section className="space-y-3" aria-label="Recent and pinned projects">
            <p className="text-sm text-secondary">Choose a project, then start blank or from an issue or pull request.</p>
            {launcherLoading && <p role="status" className="text-sm text-muted">Loading projects…</p>}
            {launcherError && (
              <div className="space-y-2">
                <p role="alert" className="text-sm text-critical">Could not load recent activity. {launcherError}</p>
                <button type="button" className="btn-secondary text-xs" onClick={() => setLauncherRetry((retryEpoch) => retryEpoch + 1)}>Retry</button>
              </div>
            )}
            {!launcherLoading && profiles.length === 0 && (
              <p className="rounded-md border border-subtle p-3 text-sm text-secondary">No project is configured yet.</p>
            )}
            <div className="divide-y divide-subtle overflow-hidden rounded-lg border border-subtle">
              {launcherProjects.map((candidate) => {
                const pinned = pinnedProjects.includes(candidate.name);
                return (
                  <div key={candidate.name} className="flex items-center gap-1 p-1.5">
                    <button type="button" onClick={() => chooseProject(candidate.name, 'blank')}
                      className="min-w-0 flex-1 rounded-md px-2 py-2 text-left hover:bg-white/5">
                      <span className="block truncate text-sm font-medium text-primary">{candidate.display_name || candidate.name}</span>
                      <span className="block truncate text-[11px] text-muted">{candidate.repo}</span>
                    </button>
                    <button type="button" onClick={() => chooseProject(candidate.name, 'issue')} disabled={candidate.remote}
                      className="touch-target rounded-md p-2 text-muted hover:bg-white/5 hover:text-primary disabled:opacity-30"
                      aria-label={`Start from an issue in ${candidate.display_name || candidate.name}`} title={candidate.remote ? 'Issue lookup runs on the central node' : 'Start from an issue'}>
                      <CircleDot size={15} aria-hidden="true" />
                    </button>
                    <button type="button" onClick={() => chooseProject(candidate.name, 'pr')} disabled={candidate.remote}
                      className="touch-target rounded-md p-2 text-muted hover:bg-white/5 hover:text-primary disabled:opacity-30"
                      aria-label={`Start from a pull request in ${candidate.display_name || candidate.name}`} title={candidate.remote ? 'Pull request lookup runs on the central node' : 'Start from a pull request'}>
                      <GitPullRequest size={15} aria-hidden="true" />
                    </button>
                    <button type="button" onClick={() => togglePinnedProject(candidate.name)} aria-pressed={pinned}
                      className="touch-target rounded-md p-2 text-muted hover:bg-white/5 hover:text-primary"
                      aria-label={`${pinned ? 'Unpin' : 'Pin'} ${candidate.display_name || candidate.name}`}>
                      <Star size={15} className={pinned ? 'fill-amber-400 text-amber-400' : ''} aria-hidden="true" />
                    </button>
                  </div>
                );
              })}
            </div>
            <div className="flex items-center justify-between gap-2">
              <button type="button" onClick={() => chooseProject(currentProfile, 'blank')} className="btn-primary text-xs inline-flex items-center gap-1.5">
                <MessageSquare size={13} aria-hidden="true" /> New chat
              </button>
              {onViewAllProjects && (
                <button type="button" onClick={() => { dialog.current?.close(); onViewAllProjects(); }} className="text-xs text-accent hover:underline">
                  View all projects
                </button>
              )}
            </div>
          </section>
        ) : <>

        {/* Mode: a blank session, grab an issue into a chat (branch for
            it, mark it in progress, seed the conversation with its body),
            or open a read-only chat seeded with a PR. */}
        <div className="flex gap-1.5" role="tablist" aria-label="Chat source">
          <button
            type="button"
            role="tab"
            aria-selected={mode === 'blank'}
            onClick={() => setMode('blank')}
            className={`flex-1 disabled:opacity-50 rounded-md px-2 py-1.5 text-xs ${mode === 'blank' ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}
          >
            Blank chat
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === 'issue'}
            disabled={remoteProject}
            onClick={() => setMode('issue')}
            className={`flex-1 disabled:opacity-50 rounded-md px-2 py-1.5 text-xs inline-flex items-center justify-center gap-1.5 ${mode === 'issue' ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}
          >
            <CircleDot size={12} aria-hidden="true" /> From issue
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === 'pr'}
            disabled={remoteProject}
            onClick={() => setMode('pr')}
            className={`flex-1 disabled:opacity-50 rounded-md px-2 py-1.5 text-xs inline-flex items-center justify-center gap-1.5 ${mode === 'pr' ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}
          >
            <GitPullRequest size={12} aria-hidden="true" /> From PR
          </button>
        </div>

        {remoteProject && <p className="text-sm text-secondary">Use a blank chat for projects on another node. Issue and PR chat setup runs on the central node.</p>}
        {mode !== 'blank' && (
          <label className="block space-y-1 text-sm text-secondary">
            <span>Filter {mode === 'issue' ? 'issues' : 'pull requests'}</span>
            <input type="search" value={query} onChange={(event) => setQuery(event.target.value)}
              placeholder={mode === 'issue' ? 'Number, title or label' : 'Number, title, branch or author'}
              className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-base text-primary placeholder:text-secondary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" />
          </label>
        )}

        {mode === 'issue' && (
          <section className="space-y-2">
            <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
              <CircleDot size={13} aria-hidden="true" /> Issue
            </h3>
            {issuesLoading && <p className="text-xs text-muted">Loading issues…</p>}
            {issuesError && (
              <div className="space-y-2">
                <p role="alert" className="text-sm text-critical">Could not load issues. {issuesError}</p>
                <button type="button" className="btn-secondary text-xs" onClick={() => setSourceRetry((attempt) => attempt + 1)}>Retry issues</button>
              </div>
            )}
            <div className="grid gap-1 max-h-40 overflow-y-auto">
              {!issuesLoading && !issuesError && <BoundedCollection key={project} items={issues} query={query} label="issues"
                emptyMessage="No open issues for this project."
                searchText={(candidate) => `#${candidate.number} ${candidate.title} ${candidate.labels.join(' ')}`}
                isSelected={(candidate) => issue?.number === candidate.number}>
                {(candidate) => (
                  <button
                    key={candidate.number}
                    type="button"
                    onClick={() => { setIssue(candidate); void startFromSource('issue', candidate.number); }}
                    disabled={creating || !backend}
                    aria-pressed={issue?.number === candidate.number}
                    className={`rounded-md px-3 py-2 text-left ${issue?.number === candidate.number ? 'bg-accent/15 border border-accent/40' : 'border border-transparent hover:bg-white/5'}`}
                  >
                    <span className="block text-sm font-medium text-primary truncate">#{candidate.number} {candidate.title}</span>
                    {candidate.labels.length > 0 && (
                      <span className="block text-[11px] text-muted truncate">{candidate.labels.join(', ')}</span>
                    )}
                  </button>
                )}
              </BoundedCollection>}
            </div>
            {issue && (
              <p className="text-[11px] text-muted">
                Branches <span className="font-mono">gah/issue/…-{issue.number}</span>, marks #{issue.number} in progress, opens the chat seeded with the issue.
              </p>
            )}
          </section>
        )}

        {mode === 'pr' && (
          <section className="space-y-2">
            <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
              <GitPullRequest size={13} aria-hidden="true" /> Pull request
            </h3>
            {prsLoading && <p className="text-xs text-muted">Loading pull requests…</p>}
            {prsError && (
              <div className="space-y-2">
                <p role="alert" className="text-sm text-critical">Could not load pull requests. {prsError}</p>
                <button type="button" className="btn-secondary text-xs" onClick={() => setSourceRetry((attempt) => attempt + 1)}>Retry pull requests</button>
              </div>
            )}
            <div className="grid gap-1 max-h-40 overflow-y-auto">
              {!prsLoading && !prsError && <BoundedCollection key={project} items={prs} query={query} label="pull requests"
                emptyMessage="No open pull requests for this project."
                searchText={(candidate) => `#${candidate.number} ${candidate.title} ${candidate.headRefName ?? ''} ${candidate.author ?? ''}`}
                isSelected={(candidate) => pr?.number === candidate.number}>
                {(candidate) => (
                  <button
                    key={candidate.number}
                    type="button"
                    onClick={() => { setPr(candidate); void startFromSource('pr', candidate.number); }}
                    disabled={creating || !backend}
                    aria-pressed={pr?.number === candidate.number}
                    className={`rounded-md px-3 py-2 text-left ${pr?.number === candidate.number ? 'bg-accent/15 border border-accent/40' : 'border border-transparent hover:bg-white/5'}`}
                  >
                    <span className="block text-sm font-medium text-primary truncate">#{candidate.number} {candidate.title}</span>
                    <span className="block text-[11px] text-muted truncate">
                      {[
                        candidate.author,
                        candidate.isDraft ? 'draft' : null,
                        candidate.reviewState ? candidate.reviewState.toLowerCase().replaceAll('_', ' ') : null
                      ].filter((part) => part !== null && part.length > 0).join(' · ')}
                    </span>
                  </button>
                )}
              </BoundedCollection>}
            </div>
            {pr && (
              <p className="text-[11px] text-muted">
                Opens the chat seeded with PR #{pr.number} — read-only: no branch is created and the PR is not modified.
              </p>
            )}
          </section>
        )}

        <section className="space-y-2">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
            <FolderGit2 size={13} aria-hidden="true" /> Project
          </h3>
          <div className="grid gap-1">
            {profiles.map((p) => (
              <button
                key={p.name}
                type="button"
                onClick={() => setProject(p.name)}
                className={`rounded-md px-3 py-2 text-left ${project === p.name ? 'bg-accent/15 border border-accent/40' : 'border border-transparent hover:bg-white/5'}`}
              >
                <span className="block text-sm font-medium text-primary">{p.display_name || p.name}</span>
                <span className="block text-[11px] text-muted truncate">{p.repo}</span>
              </button>
            ))}
            {profiles.length === 0 && <p className="text-xs text-muted">No configured profiles. Import one from the rail below.</p>}
          </div>
        </section>

        {mode === 'blank' ? <>
          <ChatNodePicker {...nodeSnapshot} value={nodeId} disabled={creating}
            onChange={nodeId => setNodeChoice({ project, nodeId })} />
          <p className="text-sm text-secondary">Each node uses its own checkout; files do not move.</p>
        </> : <p className="text-sm text-secondary">Issue and PR chat setup runs on the central node.</p>}

        <section className="space-y-2">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
            <Cpu size={13} aria-hidden="true" /> Provider / model
          </h3>
          <div className="grid grid-cols-3 gap-1.5">
            {implementedBackends.map((b) => (
              <button
                key={b.id}
                type="button"
                onClick={() => setBackend(b.id)}
                className={`rounded-md px-2 py-2 text-sm ${backend === b.id ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}
              >
                {b.displayName}
              </button>
            ))}
          </div>
          {implementedBackends.length === 0 && (
            <p role="alert" className="text-sm text-critical">No chat provider is available. Configure a provider before starting a chat.</p>
          )}
          {models.length > 0 && (
            <select
              value={model ?? ''}
              onChange={(e) => setModel(e.target.value || null)}
              className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary"
              aria-label="Model"
            >
              {models.map((m) => (
                <option key={m.id} value={m.id}>{m.name}</option>
              ))}
            </select>
          )}
          {mode === 'blank' && backendInstances.length > 0 && (
            <label className="block space-y-1 text-xs text-secondary">Account
              <select value={backendInstance ?? ''} onChange={(event) => setBackendInstance(event.target.value || null)} className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-primary">
                <option value="">Default provider login</option>
                {backendInstances.map((instance) => (
                  <option key={instance.backend_instance} value={instance.backend_instance}>
                    {instance.account_label ?? instance.backend_instance} · {instance.backend_instance}
                  </option>
                ))}
              </select>
            </label>
          )}
          {backend && models.length === 0 && (
            <p className="text-[11px] text-muted">This provider uses its default model.</p>
          )}
        </section>

        {mode === 'blank' && (
          <section className="space-y-2">
            <label htmlFor="new-chat-title" className="block text-xs font-medium text-secondary">Chat name</label>
            <input
              id="new-chat-title"
              type="text"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Fix the retry loop"
              required
              className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-base text-primary"
            />
          </section>
        )}

        {error && <p className="text-xs text-red-400">{error}</p>}

        <div className="flex justify-end gap-2 pt-1">
          {launcher && <button type="button" onClick={() => setStage('launcher')} className="btn-secondary mr-auto text-xs">Back</button>}
          <button type="button" onClick={() => dialog.current?.close()} className="btn-secondary text-xs">Cancel</button>
          <button
            type="button"
            onClick={create}
            disabled={creating || !backend || profiles.length === 0 || (mode === 'blank' && (!title.trim() || !nodeReady)) || (mode !== 'blank' && remoteProject) || (mode === 'issue' && !issue) || (mode === 'pr' && !pr)}
            className="btn-primary text-xs"
          >
            {creating ? 'Creating…' : 'Start chat'}
          </button>
        </div>
        </>}
      </div>
    </dialog>
  );
}
