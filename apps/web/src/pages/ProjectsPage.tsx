import { useEffect, useState } from 'react';
import { CircleDot, ExternalLink, GitPullRequest, MessageSquare, Plus } from 'lucide-react';
import type { ChatIssueSummary, ChatPrSummary, ChatSessionSummary } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { toChatProfile, useChatProfiles } from '../hooks/useChatProfiles.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { ProjectRail } from '../components/ProjectRail.js';
import { NewChatModal } from '../components/NewChatModal.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { BoundedCollection } from '../components/BoundedCollection.js';
import { DEFAULT_CONVERSATION_ID, updateNavigation, type Page } from '../lib/navigationState.js';
import { gahApi } from '../api/client.js';
import type { ManagerBackendInfo } from '@git-agent-harness/contracts';

/**
 * Where conversations come from (#1199): every project, its open and
 * archived chats, and the issues and pull requests still waiting for one.
 *
 * The chat page stays a blank page for a new conversation; anything that
 * needs choosing — which repository, which issue, which archived thread —
 * is chosen here and hands the chat page a session id.
 */
export function ProjectsPage({ onNavigate }: { onNavigate: (page: Page) => void }) {
  const { isConnected, reconnectSeq } = useWebSocket();
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const setProfileOverride = useUiStore((s) => s.setProfileOverride);
  const profile = profileOverride ?? wsProfile ?? 'gah';
  const [profiles, setProfiles] = useChatProfiles(reconnectSeq);
  const current = profiles.find((candidate) => candidate.name === profile);

  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [sessionsError, setSessionsError] = useState(false);
  const [issues, setIssues] = useState<ChatIssueSummary[]>([]);
  const [prs, setPrs] = useState<ChatPrSummary[]>([]);
  const [workLoading, setWorkLoading] = useState(true);
  const [workError, setWorkError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [retryEpoch, setRetryEpoch] = useState(0);
  const [backends, setBackends] = useState<ManagerBackendInfo[]>([]);
  const [backend, setBackend] = useState<string>('');
  const [starting, setStarting] = useState<string | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [newChatOpen, setNewChatOpen] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);

  const remote = current?.remote ?? false;

  const refreshSessions = () => {
    gahApi.getChatSessions(profile)
      .then(({ sessions }) => { setSessions(sessions); setSessionsError(false); })
      .catch(() => setSessionsError(true));
  };

  useEffect(() => {
    setSessions([]);
    refreshSessions();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, reconnectSeq]);

  useEffect(() => {
    let cancelled = false;
    gahApi.getManagerChatSettings()
      .then((settings) => {
        if (cancelled) return;
        setBackends(settings.availableBackends);
        setBackend(settings.profileOverrides[profile] ?? settings.defaultBackend);
      })
      .catch(() => { if (!cancelled) setBackends([]); });
    return () => { cancelled = true; };
  }, [profile, reconnectSeq]);

  // Issues and PRs are the project's incoming work; a remote project's
  // provider calls run on its own node, so they are not offered here.
  useEffect(() => {
    let cancelled = false;
    setIssues([]);
    setPrs([]);
    setWorkError(null);
    if (remote) { setWorkLoading(false); return; }
    setWorkLoading(true);
    void Promise.all([
      gahApi.getChatIssues(profile).then(({ issues }) => issues),
      gahApi.getChatPrs(profile).then(({ prs }) => prs)
    ]).then(([issues, prs]) => {
      if (cancelled) return;
      setIssues(issues);
      setPrs(prs);
      setLastUpdated(Date.now());
    }).catch((error) => {
      if (!cancelled) setWorkError(error instanceof Error ? error.message : String(error));
    }).finally(() => { if (!cancelled) setWorkLoading(false); });
    return () => { cancelled = true; };
  }, [profile, remote, retryEpoch, reconnectSeq]);

  useWsReconnectRefresh(() => { refreshSessions(); setRetryEpoch((value) => value + 1); });
  // Chats created elsewhere (another tab, the API, a worker) show up here
  // without a manual refresh.
  useAutoRefresh(() => refreshSessions(), 5_000);

  /** Hand the chat page a conversation: the project follows the selection
   * so the chat page opens against the right repository. */
  const openChat = (sessionId: string | null) => {
    setProfileOverride(profile);
    updateNavigation({ profile, chat: sessionId ?? DEFAULT_CONVERSATION_ID });
    onNavigate('chat');
  };

  const startFrom = async (kind: 'issue' | 'pr', number: number) => {
    if (!backend || starting) return;
    setStarting(`${kind}-${number}`);
    setStartError(null);
    try {
      const { session } = kind === 'issue'
        ? await gahApi.startChatFromIssue(profile, number, backend, null)
        : await gahApi.startChatFromPr(profile, number, backend, null);
      openChat(session.id);
    } catch (error) {
      setStartError(error instanceof Error ? error.message : String(error));
    } finally {
      setStarting(null);
    }
  };

  const projectLabel = current?.display_name || current?.name || profile;

  return (
    <div className="flex min-h-0 flex-col">
      <PageHeader
        title="Projects"
        description="Every repository GAH can work in, the chats it already holds, and the work still waiting for one."
        lastUpdated={lastUpdated}
        onRefresh={() => { refreshSessions(); setRetryEpoch((value) => value + 1); }}
        refreshing={workLoading}
        actions={
          <button type="button" onClick={() => setNewChatOpen(true)} disabled={!isConnected || profiles.length === 0} className="btn-primary">
            <Plus size={15} aria-hidden="true" /> New chat
          </button>
        }
      />

      <div className="grid min-w-0 items-start gap-4 xl:grid-cols-[19rem_minmax(0,1fr)]">
        {/* The rail hugs its content and scrolls on its own once the chat
            list outgrows the viewport. */}
        <div className="min-w-0 xl:sticky xl:top-4 xl:max-h-[calc(100dvh-7rem)] xl:overflow-y-auto">
        <ProjectRail
          currentProfile={profile}
          profiles={profiles.map((project) => ({ ...project, name: project.catalogName ?? project.name }))}
          sessions={sessions}
          selectedSessionId={null}
          onSelect={(selected) => setProfileOverride(selected)}
          onSessionSelect={openChat}
          sessionsError={sessionsError}
          onRetrySessions={refreshSessions}
          onProjectAdded={(project) => {
            setProfiles((previous) => [
              ...previous.filter((item) => item.name !== project.chat_profile),
              toChatProfile(project)
            ]);
          }}
        />
        </div>

        <section className="min-w-0 space-y-4" aria-label={`${projectLabel} work`}>
          <div className="card-padded">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0">
                <h3 className="truncate text-base font-semibold text-primary">{projectLabel}</h3>
                <p className="truncate text-xs text-muted">{current?.repo ?? 'No repository resolved for this profile.'}</p>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {current?.web_url && (
                  <a href={current.web_url} target="_blank" rel="noopener noreferrer" className="btn-secondary text-xs">
                    <ExternalLink size={13} aria-hidden="true" /> Repository
                  </a>
                )}
                <button type="button" onClick={() => openChat(null)} className="btn-secondary text-xs">
                  <MessageSquare size={13} aria-hidden="true" /> Default conversation
                </button>
              </div>
            </div>
          </div>

          {startError && <p role="alert" className="text-sm text-critical">{startError}</p>}

          {remote ? (
            <p className="card-padded text-sm text-secondary">
              This project lives on another node. Its issues and pull requests are read through that node — start a blank chat here instead.
            </p>
          ) : (
            <>
              <label className="block">
                <span className="sr-only">Filter issues and pull requests</span>
                <input
                  type="search"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  aria-label="Filter issues and pull requests"
                  placeholder="Filter by number, title, label, branch or author"
                  className="w-full rounded-md border border-subtle bg-raised px-3 py-2 text-base text-primary placeholder:text-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent sm:text-sm"
                />
              </label>

              {workError && (
                <div role="alert" className="card-padded space-y-2 border-critical/30">
                  <p className="text-sm text-critical">Could not read this project's issues and pull requests. {workError}</p>
                  <button type="button" onClick={() => setRetryEpoch((value) => value + 1)} className="btn-secondary text-xs">Retry</button>
                </div>
              )}

              <WorkList
                title="Open issues"
                icon={CircleDot}
                accent="text-accent"
                loading={workLoading}
                empty="No open issues. Start a blank chat instead."
                items={issues.map((issue) => ({
                  key: `issue-${issue.number}`,
                  number: issue.number,
                  title: issue.title,
                  url: issue.url,
                  meta: issue.labels.join(' · '),
                  search: `#${issue.number} ${issue.title} ${issue.labels.join(' ')}`
                }))}
                actionLabel="Start chat"
                hint="Branches for the issue, marks it in progress, and opens the chat seeded with it."
                busyKey={starting}
                disabled={!backend || !isConnected}
                query={query}
                onStart={(number) => void startFrom('issue', number)}
              />

              <WorkList
                title="Open pull requests"
                icon={GitPullRequest}
                accent="text-purple-300"
                loading={workLoading}
                empty="No open pull requests."
                items={prs.map((pr) => ({
                  key: `pr-${pr.number}`,
                  number: pr.number,
                  title: pr.title,
                  url: pr.url,
                  meta: [pr.author, pr.headRefName, pr.isDraft ? 'draft' : null].filter((part): part is string => Boolean(part)).join(' · '),
                  search: `#${pr.number} ${pr.title} ${pr.headRefName ?? ''} ${pr.author ?? ''}`
                }))}
                actionLabel="Review in chat"
                hint="Read-only: no branch is created and nothing at the provider changes."
                busyKey={starting}
                disabled={!backend || !isConnected}
                query={query}
                onStart={(number) => void startFrom('pr', number)}
              />
            </>
          )}
        </section>
      </div>

      <NewChatModal
        open={newChatOpen}
        currentProfile={profile}
        profiles={profiles}
        backends={backends}
        onClose={() => setNewChatOpen(false)}
        onCreated={(createdProfile, sessionId) => {
          setProfileOverride(createdProfile);
          updateNavigation({ profile: createdProfile, chat: sessionId });
          onNavigate('chat');
        }}
      />
    </div>
  );
}

interface WorkItem {
  key: string;
  number: number;
  title: string;
  url: string | null;
  meta: string;
  search: string;
}

/** One provider work queue: bounded rows, each one click from a chat. */
function WorkList({
  title, icon: Icon, accent, items, loading, empty, actionLabel, hint, busyKey, disabled, query, onStart
}: {
  title: string;
  icon: typeof CircleDot;
  accent: string;
  items: WorkItem[];
  loading: boolean;
  empty: string;
  actionLabel: string;
  hint: string;
  busyKey: string | null;
  disabled: boolean;
  query: string;
  onStart: (number: number) => void;
}) {
  return (
    <section className="card-padded" aria-label={title}>
      <div className="flex items-baseline justify-between gap-2 pb-1">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
          <Icon size={13} className={accent} aria-hidden="true" /> {title}
        </h3>
        <span className="text-xs tabular-nums text-muted">{loading ? '' : items.length}</span>
      </div>
      <p className="pb-2 text-[11px] text-muted">{hint}</p>
      {loading ? (
        <ul className="space-y-1.5" aria-label={`Loading ${title.toLowerCase()}`}>
          {Array.from({ length: 3 }, (_, index) => (
            <li key={index} className="h-12 animate-pulse rounded-md border border-subtle bg-raised/60" />
          ))}
        </ul>
      ) : (
        <div className="space-y-1">
          <BoundedCollection
            items={items}
            query={query}
            label={title.toLowerCase()}
            emptyMessage={empty}
            searchText={(item) => item.search}
            isSelected={() => false}
          >
            {(item) => (
              <div key={item.key} className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-white/5">
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm text-primary">
                    <span className={`font-mono ${accent}`}>#{item.number}</span> {item.title}
                  </span>
                  {item.meta && <span className="block truncate text-[11px] text-muted">{item.meta}</span>}
                </span>
                {item.url && (
                  <a
                    href={item.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="touch-target shrink-0 rounded-md p-1.5 text-muted hover:bg-white/5 hover:text-primary"
                    aria-label={`Open #${item.number} at the provider`}
                  >
                    <ExternalLink size={13} aria-hidden="true" />
                  </a>
                )}
                <button
                  type="button"
                  onClick={() => onStart(item.number)}
                  disabled={disabled || busyKey !== null}
                  className="btn-secondary shrink-0 text-xs"
                >
                  {busyKey === item.key ? 'Starting…' : actionLabel}
                </button>
              </div>
            )}
          </BoundedCollection>
        </div>
      )}
    </section>
  );
}
