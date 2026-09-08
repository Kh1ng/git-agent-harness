import { useEffect, useMemo, useState } from 'react';
import { ChevronRight, FolderGit2 } from 'lucide-react';
import type { ChatNodeInfo, ChatSessionSummary, ProfileSummary, ProjectSummary } from '@git-agent-harness/contracts';
import { formatChatName } from '../lib/format.js';
import { BoundedCollection } from './BoundedCollection.js';
import { projectKey } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

/**
 * The chat page's navigator: every configured project and every conversation
 * in the selected project is one click away. Archived conversations stay
 * available behind a disclosure instead of crowding the working set.
 *
 * Local configured profiles remain chattable. Imported remote projects carry
 * their owning node and conversation identity so equal names cannot collide.
 */
export function ProjectRail({
  currentProfile,
  profiles,
  sessions,
  selectedSessionId,
  onSelect,
  onSessionSelect,
  sessionsError,
  onRetrySessions,
  onProjectAdded
}: {
  currentProfile: string;
  profiles: (ProfileSummary & Partial<Pick<ProjectSummary, 'node_id' | 'chat_profile'>>)[];
  sessions: ChatSessionSummary[];
  selectedSessionId: string | null;
  onSelect: (profile: string | null, nodeId?: string) => void;
  onSessionSelect: (sessionId: string | null) => void;
  sessionsError: boolean;
  onRetrySessions: () => void;
  onProjectAdded: (profile: ProjectSummary) => void;
}) {
  const [query, setQuery] = useState('');
  const [gitUrl, setGitUrl] = useState('');
  const [nodes, setNodes] = useState<ChatNodeInfo[]>([]);
  const [nodesError, setNodesError] = useState(false);
  const [nodeId, setNodeId] = useState('');
  const [provider, setProvider] = useState<'auto' | 'gitlab'>('auto');
  const [providerApiBase, setProviderApiBase] = useState('');
  const [providerProjectId, setProviderProjectId] = useState('');
  const gitlab = provider === 'gitlab' || /^(https:\/\/|ssh:\/\/(git@)?|git@)gitlab\.com[/:]/i.test(gitUrl.trim());
  const loadNodes = () => {
    setNodesError(false);
    return gahApi.getChatNodes().then((result) => setNodes(result.nodes)).catch(() => setNodesError(true));
  };
  useEffect(() => { void loadNodes(); }, []);
  const localNodeId = nodes.find((node) => node.role === 'central')?.nodeId || '';
  const nodeName = (id: string) => nodes.find((node) => node.nodeId === id)?.displayName || id || 'This node';
  const [reclone, setReclone] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ tone: 'error' | 'success'; text: string } | null>(null);

  const sorted = useMemo(
    () => [...profiles].sort((a, b) => (a.display_name || a.name).localeCompare(b.display_name || b.name)),
    [profiles]
  );
  const liveSessions = sessions.filter((session) => session.outcome === 'live');
  const archivedSessions = sessions.filter((session) => session.outcome !== 'live');
  const sessionClasses = (active: boolean) =>
    `block w-full rounded-md px-2 py-1.5 text-left ${active ? 'bg-accent/15' : 'hover:bg-white/5'}`;

  const importProject = async () => {
    if (!gitUrl.trim()) return;
    setSaving(true);
    setMessage(null);
    try {
      const result = await gahApi.importProject({
        gitUrl: gitUrl.trim(), reclone, ...(nodeId ? { nodeId } : {}),
        ...(gitlab ? { provider: 'gitlab', providerProjectId: providerProjectId.trim(), ...(providerApiBase.trim() ? { providerApiBase: providerApiBase.trim() } : {}) } : {})
      });
      onProjectAdded(result.project);
      onSelect(result.project.chat_profile, result.project.node_id);
      setGitUrl('');
      setReclone(false);
      setProviderProjectId('');
      const languages = result.detectedLanguages.length > 0
        ? ` Detected ${result.detectedLanguages.join(', ')}.`
        : '';
      const validation = result.validationCommands.length > 0
        ? ` Validation: ${result.validationCommands.join(', ')}.`
        : ' No validation command was detected.';
      setMessage({ tone: 'success', text: `${result.checkoutStatus} ${result.project.repo}.${languages}${validation}` });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <aside className="card p-3 xl:h-[65vh] xl:overflow-y-auto" aria-label="Chat navigation">
      <details open>
        <summary className="group flex cursor-pointer list-none items-center justify-between gap-2 rounded px-1 pb-2 text-xs font-semibold uppercase tracking-wide text-muted marker:content-none hover:text-primary">
          <span className="flex items-center gap-1">
            <ChevronRight size={12} className="transition-transform group-open:rotate-90" aria-hidden="true" />
            Projects
          </span>
          <span className="tabular-nums">{sorted.length}</span>
        </summary>

        <nav aria-label="Projects" className="space-y-1">
          {sorted.length === 0 && (
            <p className="px-2 py-3 text-xs leading-relaxed text-muted">Import a repository below to start.</p>
          )}
          {[...new Set(sorted.map((project) => project.node_id || localNodeId))].map((owner) => (
            <section key={owner} aria-label={`Projects on ${nodeName(owner)}`} className="space-y-1 pt-2 first:pt-0">
              <h3 className="px-2 text-xs font-medium text-secondary break-words">{nodeName(owner)}</h3>
              {sorted.filter((project) => (project.node_id || localNodeId) === owner).map((project) => {
                const selection = project.chat_profile || project.name;
                const active = selection === currentProfile;
                return (
                  <button key={projectKey({ node_id: owner, name: project.name })} type="button"
                    onClick={() => onSelect(selection, owner || undefined)}
                    className={`block w-full rounded-md px-2 py-2 text-left ${active ? 'bg-accent/15' : 'hover:bg-white/5'}`}
                    aria-current={active ? 'page' : undefined}>
                    <span className="block truncate text-sm font-medium text-primary">{project.display_name || project.name}</span>
                    <span className="block truncate text-xs text-muted">{project.repo}</span>
                    <span className="mt-1 block truncate text-xs text-secondary">Runs on {nodeName(owner)}</span>
                  </button>
                );
              })}
            </section>
          ))}
        </nav>
      </details>

      <section className="mt-4 border-t border-subtle pt-3" aria-labelledby="chat-list-title">
        <div className="flex items-center justify-between gap-2 px-1 pb-2">
          <h3 id="chat-list-title" className="text-xs font-semibold uppercase tracking-wide text-muted">Chats</h3>
          {sessionsError ? (
            <button type="button" onClick={onRetrySessions} className="text-[11px] text-amber-300 hover:text-primary">Retry</button>
          ) : (
            <span className="text-xs tabular-nums text-muted">{liveSessions.length + 1}</span>
          )}
        </div>
        <label className="mb-2 block space-y-1 text-sm text-secondary">
          <span>Filter chats and archive</span>
          <input type="search" value={query} onChange={(event) => setQuery(event.target.value)}
            placeholder="Name, number or branch"
            className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-base text-primary placeholder:text-secondary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" />
        </label>
        <nav aria-label="Chats" className="space-y-1">
          <BoundedCollection<ChatSessionSummary | null> key={currentProfile} items={[null, ...liveSessions]} query={query}
            label="chats" emptyMessage="No chats yet."
            searchText={(session) => session ? `${formatChatName(session)} ${session.branch} ${session.prNumber ?? ''}` : 'Default conversation'}
            isSelected={(session) => (session?.id ?? null) === selectedSessionId}>
            {(session) => (
              <button key={session?.id ?? 'default'} type="button" onClick={() => onSessionSelect(session?.id ?? null)}
                className={sessionClasses((session?.id ?? null) === selectedSessionId)}
                aria-current={(session?.id ?? null) === selectedSessionId ? 'page' : undefined}>
                <span className="block truncate text-sm text-primary">{session ? formatChatName(session) : 'Default conversation'}</span>
                {session && <span className="block truncate text-[11px] text-muted">{session.branch}</span>}
              </button>
            )}
          </BoundedCollection>
        </nav>

        {archivedSessions.length > 0 && (
          <details className="mt-2" open={archivedSessions.some((session) => session.id === selectedSessionId) || undefined}>
            <summary className="cursor-pointer select-none rounded px-2 py-1 text-xs text-muted hover:bg-white/5 hover:text-primary">
              Archived ({archivedSessions.length})
            </summary>
            <nav aria-label="Archived chats" className="mt-1 space-y-1">
              <BoundedCollection key={currentProfile} items={archivedSessions} query={query} label="archived chats"
                emptyMessage="No archived chats."
                searchText={(session) => `${formatChatName(session)} ${session.branch} ${session.prNumber ?? ''}`}
                isSelected={(session) => session.id === selectedSessionId}>
                {(session) => (
                  <button key={session.id} type="button" onClick={() => onSessionSelect(session.id)}
                    className={sessionClasses(session.id === selectedSessionId)}
                    aria-current={session.id === selectedSessionId ? 'page' : undefined}>
                    <span className="block truncate text-sm text-secondary">{formatChatName(session)}</span>
                    <span className="block truncate text-[11px] text-muted">{session.branch}</span>
                  </button>
                )}
              </BoundedCollection>
            </nav>
          </details>
        )}
      </section>

      <div className="mt-4 border-t border-subtle pt-3 space-y-2">
        <details className="pt-1">
          <summary className="cursor-pointer select-none text-xs font-medium text-secondary hover:text-primary">Import from Git</summary>
          <div className="mt-2 space-y-2">
            <label className="block space-y-1 text-xs text-secondary">
              <span>Import on</span>
              <select value={nodeId} onChange={(event) => setNodeId(event.target.value)} disabled={saving}
                className="w-full rounded-md border border-subtle bg-raised px-2 py-2 text-base text-primary">
                <option value="">{nodeName(localNodeId)} (central)</option>
                {nodes.filter((node) => node.role === 'worker').map((node) => <option key={node.nodeId} value={node.nodeId}>{node.displayName}</option>)}
              </select>
            </label>
            {nodesError && <p role="alert" className="text-xs text-critical">Could not load worker nodes. <button type="button" onClick={loadNodes} className="underline">Retry node list</button></p>}
            <label className="block space-y-1 text-xs text-secondary" htmlFor="project-git-url">
              <span>Git repository URL</span>
              <input id="project-git-url" type="text" value={gitUrl} onChange={(event) => setGitUrl(event.target.value)} disabled={saving}
                placeholder="https://github.com/owner/repo"
                className="w-full rounded-md border border-subtle bg-raised px-2 py-2 text-base text-primary placeholder:text-muted" />
            </label>
            <label className="block space-y-1 text-xs text-secondary">
              <span>Repository provider</span>
              <select value={provider} onChange={(event) => setProvider(event.target.value as 'auto' | 'gitlab')} disabled={saving}
                className="w-full rounded-md border border-subtle bg-raised px-2 py-2 text-base text-primary">
                <option value="auto">GitHub.com or GitLab.com</option>
                <option value="gitlab">GitLab, including custom hosts</option>
              </select>
            </label>
            {gitlab && <>
              <label className="block space-y-1 text-xs text-secondary">
                <span>GitLab project ID</span>
                <input value={providerProjectId} onChange={(event) => setProviderProjectId(event.target.value)} inputMode="numeric" disabled={saving}
                  aria-describedby="gitlab-project-id-hint" className="w-full rounded-md border border-subtle bg-raised px-2 py-2 text-base text-primary" />
              </label>
              <p id="gitlab-project-id-hint" className="text-xs text-muted">Copy the numeric ID from the GitLab project overview.</p>
              <label className="block space-y-1 text-xs text-secondary">
                <span>GitLab API URL (optional)</span>
                <input value={providerApiBase} onChange={(event) => setProviderApiBase(event.target.value)} disabled={saving}
                  placeholder="https://gitlab.example.com/api/v4" aria-describedby="gitlab-api-hint"
                  className="w-full rounded-md border border-subtle bg-raised px-2 py-2 text-base text-primary placeholder:text-muted" />
              </label>
              <p id="gitlab-api-hint" className="text-xs text-muted">Defaults to HTTPS on the repository host. Set this for a custom API port or path.</p>
            </>}
            <label className="flex items-start gap-2 text-[11px] leading-snug text-muted">
              <input type="checkbox" checked={reclone} onChange={(event) => setReclone(event.target.checked)} className="mt-0.5" />
              Re-clone an existing clean managed checkout
            </label>
            <button type="button" onClick={importProject} disabled={saving || !gitUrl.trim() || (gitlab && !/^[1-9][0-9]*$/.test(providerProjectId.trim()))} className="btn-primary w-full !min-h-0 text-xs">
              <FolderGit2 size={14} aria-hidden="true" />
              {saving ? 'Working…' : 'Import repository'}
            </button>
          </div>
        </details>

        {message && (
          <p role={message.tone === 'error' ? 'alert' : 'status'} className={`text-[11px] leading-relaxed ${message.tone === 'error' ? 'text-red-400' : 'text-secondary'}`}>
            {message.text}
          </p>
        )}
      </div>
    </aside>
  );
}
