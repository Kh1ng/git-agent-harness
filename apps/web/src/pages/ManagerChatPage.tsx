import { useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { Send, Square, MessageSquare, GitBranch, Plus, Archive, Wrench, ShieldAlert, MonitorPlay, X, ExternalLink, HardDrive, RefreshCw, Sparkles, GitPullRequest, GitCommit } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { NewChatModal } from '../components/NewChatModal.js';
import { ChatSessionPicker } from '../components/ChatSessionPicker.js';
import { ProviderPicker, type ProviderSelection, type ProviderPickerProps } from '../components/ProviderPicker.js';
import { ProjectRail } from '../components/ProjectRail.js';
import { gahApi } from '../api/client.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type {
  ManagerChatTurn,
  ManagerCommandInfo,
  ManagerModelInfo,
  ManagerReasoningEffortInfo,
  ProfileSummary,
  ManagerBackendInfo,
  ChatSessionSummary,
  ChatSessionProjectGroup,
  ChatPreviewInfo,
  ChatReclaimResult,
  SkillBindingSummary,
  ChatPrSummary,
  ChatIssueSummary
} from '@git-agent-harness/contracts';

interface ChatTurn {
  role: 'user' | 'assistant' | 'system' | 'error' | 'tool';
  text: string;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
  /** Optimistic mid-turn steer, removed again if the backend rejects it. */
  steeringRequestId?: string;
  /** Present on tool turns (slice 3): structured info for the activity card. */
  tool?: {
    toolCallId: string;
    name: string | null;
    title: string;
    kind: string | null;
    status: 'pending' | 'completed' | 'failed';
    locations: string[];
    summary: string | null;
  };
}

interface PendingRequest {
  id: string;
  profile: string;
}

interface StreamingTurn {
  turn: number;
  text: string;
}

interface LivePermission {
  permissionId: string;
  title: string;
  options: { optionId: string; name: string; kind: string }[];
  locations: string[];
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${unit}`;
}

function activeChatSessions(sessions: ChatSessionSummary[]): ChatSessionSummary[] {
  return sessions.filter((session) => session.archivedAt === null);
}

function fromServerTurn(turn: ManagerChatTurn): ChatTurn {
  return {
    role: turn.role === 'assistant' ? 'assistant' : turn.role,
    text: turn.text,
    backend: turn.backend,
    model: turn.model,
    tool: turn.tool
  };
}

function MarkdownMessage({ text }: { text: string }) {
  return (
    <div className="chat-markdown">
      <ReactMarkdown
        components={{
          a: ({ node: _node, ...props }) => (
            <a {...props} target="_blank" rel="noreferrer" />
          )
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}

/** WP3: the preview URL comes from the server (node address + allocated
 * port), but the client never trusts it blindly -- validate the exact
 * shape (plain http, host, numeric port) before it reaches an href or an
 * iframe src, so a tampered value can't redirect elsewhere. */
function safePreviewUrl(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'http:') return null;
    if (!/^\d+$/.test(parsed.port) || parsed.port === '') return null;
    if (parsed.username || parsed.password || parsed.search || parsed.hash) return null;
    return `${parsed.protocol}//${parsed.host}${parsed.pathname}`;
  } catch {
    return null;
  }
}

/** Slice 3: one agent activity card -- what the agent is doing, which files
 * it touched, and a bounded output summary once it finishes. */
function ToolCallCard({ tool }: { tool: NonNullable<ChatTurn['tool']> }) {
  const [open, setOpen] = useState(false);
  const statusColor = tool.status === 'failed'
    ? 'text-red-400'
    : tool.status === 'completed'
      ? 'text-emerald-400'
      : 'text-muted animate-pulse';
  const statusLabel = tool.status === 'pending' || tool.status === 'completed' ? tool.status : tool.status;
  return (
    <div className="w-full max-w-[80%] rounded-md border border-subtle bg-raised/60 text-xs">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left hover:bg-white/5"
        aria-expanded={open}
      >
        <Wrench size={12} className="text-muted shrink-0" aria-hidden="true" />
        <span className="min-w-0 truncate text-secondary flex-1">{tool.title}</span>
        {tool.locations.length > 0 && (
          <span className="text-[10px] text-muted truncate max-w-[30%]" title={tool.locations.join(', ')}>
            {tool.locations.length === 1 ? tool.locations[0].split('/').pop() : `${tool.locations.length} files`}
          </span>
        )}
        <span className={`shrink-0 text-[10px] uppercase tracking-wide ${statusColor}`}>{statusLabel}</span>
      </button>
      {open && (
        <div className="px-2.5 pb-2 pt-1 border-t border-subtle space-y-1">
          {tool.name && <p className="text-[10px] text-muted font-mono">{tool.name}{tool.kind ? ` · ${tool.kind}` : ''}</p>}
          {tool.locations.length > 0 && (
            <ul className="text-[10px] text-muted font-mono space-y-0.5">
              {tool.locations.map((loc) => <li key={loc} className="truncate">{loc}</li>)}
            </ul>
          )}
          {tool.summary && (
            <pre className="text-[10px] text-secondary whitespace-pre-wrap break-words max-h-40 overflow-y-auto font-mono">{tool.summary}</pre>
          )}
        </div>
      )}
    </div>
  );
}

function SkillPicker({
  binding,
  busy,
  onToggle,
  onInherit
}: {
  binding: SkillBindingSummary | null;
  busy: boolean;
  onToggle: (id: string) => void;
  onInherit: () => void;
}) {
  const observed = new Map(binding?.observedSkills?.map((skill) => [skill.id, skill.version]) ?? []);
  const selected = new Set(binding?.selectedIds ?? []);
  const selectedVersions = new Map(binding?.skills.filter((skill) => selected.has(skill.id)).map((skill) => [skill.id, skill.version]) ?? []);
  const drift = binding?.observedSkills != null
    && (selectedVersions.size !== observed.size || [...selectedVersions].some(([id, version]) => observed.get(id) !== version));

  return (
    <details className="group relative">
      <summary
        className="flex min-h-9 cursor-pointer list-none items-center gap-1.5 rounded-md border border-subtle bg-raised px-2.5 py-1.5 text-xs text-secondary hover:bg-white/5 focus-visible:outline-none [&::-webkit-details-marker]:hidden"
        aria-label="Project skills"
        onClick={(event) => { if (busy) event.preventDefault(); }}
        title={busy ? 'Skill changes are disabled while a turn is in flight' : 'Choose the project skills applied to the next turn'}
      >
        <Sparkles size={13} className="text-accent" aria-hidden="true" />
        Skills · {binding?.selectedIds.length ?? 0}
        {drift && <span className="h-1.5 w-1.5 rounded-full bg-amber-400" title="Configured skills differ from the latest applied turn" />}
      </summary>
      <div className="absolute right-0 z-30 mt-1 w-64 max-w-[calc(100vw-2rem)] rounded-lg border border-subtle bg-raised p-3 shadow-xl">
        <div className="mb-2 flex items-start justify-between gap-3">
          <div>
            <p className="text-sm font-medium text-primary">Project skills</p>
            <p className="text-[11px] text-muted">
              {binding ? `${binding.source === 'profile' ? 'Project override' : 'Inherited default'} · ${binding.backend}` : 'Loading…'}
            </p>
          </div>
          {binding?.source === 'profile' && (
            <button type="button" onClick={onInherit} disabled={busy} className="text-[11px] text-accent hover:underline disabled:opacity-50">
              Use default
            </button>
          )}
        </div>
        {!binding ? (
          <p className="py-3 text-xs text-muted">Loading available skills…</p>
        ) : !binding.supported ? (
          <p className="py-3 text-xs text-muted">This backend does not support bound skills.</p>
        ) : binding.skills.length === 0 ? (
          <p className="py-3 text-xs text-muted">No compatible skills are installed in the central bank.</p>
        ) : (
          <div className="space-y-1">
            {binding.skills.map((skill) => (
              <label key={skill.id} className="flex cursor-pointer gap-2 rounded-md px-2 py-1.5 hover:bg-white/5">
                <input
                  type="checkbox"
                  checked={selected.has(skill.id)}
                  disabled={busy}
                  onChange={() => onToggle(skill.id)}
                  className="mt-0.5 accent-[rgb(var(--accent))]"
                />
                <span className="min-w-0">
                  <span className="block text-xs text-primary">{skill.displayName} <span className="font-mono text-[10px] text-muted">{skill.version}</span></span>
                  <span className="block truncate text-[11px] text-muted" title={skill.description}>{skill.description}</span>
                </span>
              </label>
            ))}
          </div>
        )}
        {binding?.observedSkills != null && (
          <p className={`mt-2 border-t border-subtle pt-2 text-[11px] ${drift ? 'text-amber-300' : 'text-muted'}`}>
            {drift ? 'Changed since the latest applied turn. The next turn uses this selection.' : 'Matches the latest applied turn.'}
          </p>
        )}
      </div>
    </details>
  );
}

/**
 * Git strip: compact view of branch, changes count, open issues/PRs with actions.
 * Renders for the active session's project when git data is available.
 */
function GitStrip({
  profile,
  status,
  issues,
  prs,
  onRefresh
}: {
  profile: string;
  status: { branch: string; changes: { status: string; path: string }[]; cwd: string } | null;
  issues: ChatIssueSummary[];
  prs: ChatPrSummary[];
  onRefresh: () => void;
}) {
  const [commitOpen, setCommitOpen] = useState(false);
  const [commitMessage, setCommitMessage] = useState('');
  const [committing, setCommitting] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);

  if (!status) return null;

  const clean = status.changes.length === 0;
  const changedFiles = status.changes.length;
  // The session's branch already maps to its PR via the settle-by-branch
  // sweep, so prefer that match; fall back to the first open PR.
  const branchPr = prs.find((pr) => pr.headRefName === status.branch) ?? prs[0] ?? null;

  const submitCommit = async () => {
    if (!commitMessage.trim() || committing) return;
    setCommitting(true);
    setCommitError(null);
    try {
      await gahApi.createGitCommit(profile, commitMessage.trim());
      setCommitMessage('');
      setCommitOpen(false);
      onRefresh();
    } catch (error) {
      setCommitError(error instanceof Error ? error.message : String(error));
    } finally {
      setCommitting(false);
    }
  };

  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3 truncate">
          <GitBranch size={14} className="text-muted shrink-0" />
          <span className="font-mono text-secondary">{status.branch}</span>
          {clean ? (
            <span className="text-muted">clean</span>
          ) : (
            <span className="flex items-center gap-1 text-amber-400">
              <span className="h-1.5 w-1.5 rounded-full bg-amber-400" />
              {changedFiles} changed
            </span>
          )}
          {prs.length > 0 && (
            <>
              <span className="text-muted">|</span>
              <span className="flex items-center gap-1">
                <GitPullRequest size={13} className="text-purple-400" />
                {prs.length}
              </span>
            </>
          )}
          {issues.length > 0 && (
            <>
              <span className="text-muted">|</span>
              <span className="flex items-center gap-1">
                <ShieldAlert size={13} className="text-blue-400" />
                {issues.length}
              </span>
            </>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={() => { setCommitOpen((v) => !v); setCommitError(null); }}
            className="flex items-center gap-1 text-muted hover:text-primary disabled:opacity-50"
            title={clean ? 'No changes to commit' : 'Commit changes'}
            disabled={clean}
          >
            <GitCommit size={13} />
            Commit
          </button>
          <button
            onClick={() => branchPr?.url && window.open(branchPr.url, '_blank', 'noopener,noreferrer')}
            className="flex items-center gap-1 text-muted hover:text-primary disabled:opacity-50"
            title={branchPr ? `View #${branchPr.number}: ${branchPr.title}` : 'No open PR for this branch'}
            disabled={!branchPr?.url}
          >
            <ExternalLink size={13} />
            View PR
          </button>
          <button
            onClick={onRefresh}
            className="text-muted hover:text-primary"
            title="Refresh git data"
          >
            <RefreshCw size={13} />
          </button>
        </div>
      </div>

      {commitOpen && (
        <div className="flex items-center gap-2 rounded-md border border-subtle bg-raised px-2 py-1.5">
          <input
            type="text"
            autoFocus
            value={commitMessage}
            onChange={(e) => setCommitMessage(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void submitCommit(); if (e.key === 'Escape') setCommitOpen(false); }}
            placeholder="Commit message"
            className="flex-1 bg-transparent text-primary text-xs focus:outline-none"
          />
          <button
            onClick={submitCommit}
            disabled={!commitMessage.trim() || committing}
            className="btn-primary text-[11px] px-2 py-1 disabled:opacity-50"
          >
            {committing ? 'Committing…' : 'Commit'}
          </button>
          {commitError && <span className="text-critical text-[11px]">{commitError}</span>}
        </div>
      )}
    </div>
  );
}

export function ManagerChatPage() {
  const { sendMessage, messages, isConnected, reconnectSeq } = useWebSocket();
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const setProfileOverride = useUiStore((s) => s.setProfileOverride);
  const profile = profileOverride ?? wsProfile ?? 'gah';
  const [availableProfiles, setAvailableProfiles] = useState<ProfileSummary[]>([]);
  const currentProfileInfo = availableProfiles.find((p) => p.name === profile);

  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [draft, setDraft] = useState('');
  const [pendingRequest, setPendingRequest] = useState<PendingRequest | null>(null);
  const [remoteTurnBusy, setRemoteTurnBusy] = useState(false);
  /** Live assistant turn (#959): a tee of the session log's chunks. The log
   * is the record; this state is just the live append. Replaced by the final
   * turn when manager.chat.reply arrives, and re-derived from history on
   * reload. */
  const [streaming, setStreaming] = useState<StreamingTurn | null>(null);
  /** Slice 3: live tool-call activity for the in-flight turn (keyed by
   * toolCallId; latest status wins). Rendered between the transcript and
   * the streaming bubble. */
  const [liveTools, setLiveTools] = useState<Record<string, NonNullable<ChatTurn['tool']>>>({});
  /** Slice 3: the live permission request, when the backend is blocked on
   * a human decision. */
  const [permission, setPermission] = useState<LivePermission | null>(null);
  /** WP3: the session's live preview + panel visibility. */
  const [preview, setPreview] = useState<ChatPreviewInfo | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewPortDraft, setPreviewPortDraft] = useState('');
  /** WP3: mixed-content detection — an HTTPS dashboard embedding an HTTP
   * preview gets blocked by the browser. A blocked frame keeps an
   * accessible-but-empty contentDocument (about:blank); a loaded frame is
   * cross-origin (different port) and access throws. */
  const [previewBlocked, setPreviewBlocked] = useState(false);
  /** Highest log seq this client has applied. Chunks at or below it are
   * already reflected in the rendered transcript (e.g. fetched via history),
   * so they're skipped -- gives exactly-once rendering for a client that
   * connects mid-turn. */
  const lastAppliedSeqRef = useRef(0);
  // The provider evicts old inbox entries to keep memory bounded. Track its
  // monotonic receive id rather than an array offset, which becomes invalid
  // as soon as the front of that array rolls over.
  const processedMessageIdRef = useRef(messages.at(-1)?.id ?? 0);
  const [historyLoaded, setHistoryLoaded] = useState(false);
  /** Which harness answers this profile's chat (#945): a stored per-profile
   * override, not client state. The header picker writes it via
   * POST /api/manager-chat/settings and the next turn uses it. */
  const [activeBackendId, setActiveBackendId] = useState<string | null>(null);
  const [availableBackends, setAvailableBackends] = useState<ManagerBackendInfo[]>([]);
  const [backendChanging, setBackendChanging] = useState(false);
  const [commands, setCommands] = useState<ManagerCommandInfo[]>([]);
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [currentModelId, setCurrentModelId] = useState<string | null>(null);
  const [reasoningEfforts, setReasoningEfforts] = useState<ManagerReasoningEffortInfo[]>([]);
  const [currentReasoningEffortId, setCurrentReasoningEffortId] = useState<string | null>(null);
  /** Context-window occupancy from the active backend's own usage_update
   * notification (issue #865, Hermes today) -- null hides the indicator. */
  const [contextUsage, setContextUsage] = useState<{ size: number; used: number } | null>(null);
  /** True once the model list fetch completed (an empty list is "this
   * backend exposes no picker", not "still loading"). */
  const [modelsLoaded, setModelsLoaded] = useState(false);
  const [modelChanging, setModelChanging] = useState(false);
  const [reasoningEffortChanging, setReasoningEffortChanging] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const processedRequestIds = useRef(new Set<string>());
  const steeringRequestIds = useRef(new Set<string>());
  const historyRequestId = useRef<string | null>(null);
  const activeProfileRef = useRef(profile);
  activeProfileRef.current = profile;
  const scrollAnchorRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  /** WP2 sessions: null = the profile's default conversation; otherwise a
   * session bound to its own worktree. */
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [sessionsError, setSessionsError] = useState(false);
  /** Cross-project picker data: every project's sessions (live + archived),
   * independent of the profile this page is currently viewing. */
  const [allSessions, setAllSessions] = useState<ChatSessionProjectGroup[]>([]);
  const [allSessionsError, setAllSessionsError] = useState(false);
  /** A cross-project picker selection (or new-chat creation) lands in the
   * same render batch as the profile switch; the [profile] effect's cleanup
   * would wipe the session id, so the effect restores it after the reset. */
  const pendingSessionRef = useRef<string | null>(null);
  const [storageOpen, setStorageOpen] = useState(false);
  const [storage, setStorage] = useState<ChatReclaimResult | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);
  const [storageLoading, setStorageLoading] = useState(false);
  const [selectedSessionIds, setSelectedSessionIds] = useState<Set<string>>(new Set());
  const [archiveBusy, setArchiveBusy] = useState(false);
  const activeSession = useMemo(
    () => sessions.find((s) => s.id === sessionId) ?? null,
    [sessions, sessionId]
  );
  const skillBackend = activeSession?.backend ?? activeBackendId;
  const [skillBinding, setSkillBinding] = useState<SkillBindingSummary | null>(null);
  const [skillBindingChanging, setSkillBindingChanging] = useState(false);
  
  // Git strip data for active profile/project
  const [gitStatus, setGitStatus] = useState<{ branch: string; changes: { status: string; path: string }[]; cwd: string } | null>(null);
  const [gitIssues, setGitIssues] = useState<ChatIssueSummary[]>([]);
  const [gitPrs, setGitPrs] = useState<ChatPrSummary[]>([]);
  const [gitLoading, setGitLoading] = useState(false);
  const [gitError, setGitError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSkillBinding(null);
    if (!skillBackend) return;
    gahApi.getSkillBindings(profile, skillBackend, sessionId)
      .then((binding) => { if (!cancelled) setSkillBinding(binding); })
      .catch(() => { if (!cancelled) setSkillBinding(null); });
    return () => { cancelled = true; };
  }, [profile, skillBackend, sessionId, turns.length]);

  useEffect(() => {
    gahApi.getProfiles().then(setAvailableProfiles).catch(() => {});
  }, []);

  const loadGitData = async () => {
    setGitLoading(true);
    setGitError(null);
    try {
      const [status, issues, prs] = await Promise.all([
        gahApi.getGitStatus(profile),
        gahApi.getChatIssues(profile).then(({ issues }) => issues).catch(() => [] as ChatIssueSummary[]),
        gahApi.getChatPrs(profile).then(({ prs }) => prs).catch(() => [] as ChatPrSummary[])
      ]);
      setGitStatus(status);
      setGitIssues(issues);
      setGitPrs(prs);
    } catch (error) {
      setGitError(error instanceof Error ? error.message : String(error));
    } finally {
      setGitLoading(false);
    }
  };

  useEffect(() => { loadGitData(); }, [profile]);

  const refreshSessions = (forProfile: string) => {
    gahApi
      .getChatSessions(forProfile)
      .then(({ sessions: fetched }) => {
        if (activeProfileRef.current !== forProfile) return;
        setSessions(activeChatSessions(fetched));
        setSessionsError(false);
      })
      .catch(() => {
        // #1002: a transient fetch failure must not wipe the rail — that made
        // every session "disappear" on a blip. Keep the last known list and
        // surface the failure instead of silently emptying.
        if (activeProfileRef.current === forProfile) setSessionsError(true);
      });
  };

  const refreshAllSessions = () => {
    gahApi
      .getAllChatSessions()
      .then(({ projects }) => {
        setAllSessions(projects);
        setAllSessionsError(false);
      })
      .catch(() => {
        // Same resilience rule as refreshSessions: keep the last known
        // groups and let the retry affordance surface the failure.
        setAllSessionsError(true);
      });
  };

  useAutoRefresh(() => { refreshSessions(profile); refreshAllSessions(); }, 5_000);
  useWsReconnectRefresh(() => { refreshSessions(profile); refreshAllSessions(); });
  // #865: Hermes pushes usage_update per turn, so the context meter is
  // polled independently of the models/reasoning-effort list (which only
  // refreshes on explicit profile/backend/model changes) to stay live
  // through an ongoing conversation. Only the default (non-session)
  // conversation shows the meter, so skip the request while a session owns
  // the header.
  useAutoRefresh(() => {
    if (activeSession) return;
    const requestedProfile = profile;
    gahApi.getManagerChatModels(profile)
      .then(({ contextUsage: usage }) => {
        if (activeProfileRef.current === requestedProfile) setContextUsage(usage ?? null);
      })
      .catch(() => {});
  }, 5_000);

  useEffect(() => {
    const pendingSession = pendingSessionRef.current;
    pendingSessionRef.current = null;
    refreshSessions(profile);
    refreshAllSessions();
    if (pendingSession !== null) setSessionId(pendingSession);
    return () => {
      setSessions([]);
      setSessionId(null);
      setStorage(null);
      setSelectedSessionIds(new Set());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile]);

  /** Picker selection: a session of another project first switches the chat
   * page to that project (the WebSocket reconnects with it), then opens the
   * session via the regular session-open path. */
  const handlePickerSelect = (sessionProfile: string, pickedSessionId: string | null) => {
    if (sessionProfile !== profile) {
      pendingSessionRef.current = pickedSessionId;
      setProfileOverride(sessionProfile);
    }
    setSessionId(pickedSessionId);
  };

  const refreshStorage = async (forProfile = profile) => {
    setStorageLoading(true);
    try {
      const result = await gahApi.getChatStorage(forProfile);
      if (activeProfileRef.current !== forProfile) return;
      setStorage(result);
      setStorageError(null);
    } catch (error) {
      if (activeProfileRef.current === forProfile) {
        setStorageError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (activeProfileRef.current === forProfile) setStorageLoading(false);
    }
  };

  useEffect(() => {
    if (storageOpen) void refreshStorage(profile);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageOpen, profile]);

  // Restore history on mount, on profile change, on session change, and
  // after a reconnect -- otherwise leaving the page (or a dropped
  // connection) silently loses the conversation even though the server
  // keeps it.
  useEffect(() => {
    setHistoryLoaded(false);
    setTurns([]);
    setPendingRequest(null);
    setRemoteTurnBusy(false);
    setStreaming(null);
    setLiveTools({});
    setPermission(null);
    setPreview(null);
    setPreviewOpen(false);
    setPreviewPortDraft('');
    steeringRequestIds.current.clear();
    if (sessionId && sessionId !== 'default') {
      gahApi
        .getChatPreview(profile, sessionId)
        .then(({ preview }) => { if (activeProfileRef.current === profile) setPreview(preview); })
        .catch(() => {});
    }
    lastAppliedSeqRef.current = 0;
    if (!isConnected) return;
    const requestId = generateRequestId();
    historyRequestId.current = requestId;
    sendMessage({
      type: 'manager.chat.historyRequest',
      requestId,
      profile,
      ...(sessionId ? { sessionId } : {})
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, sessionId, isConnected, reconnectSeq]);

  useEffect(() => {
    let cancelled = false;
    setActiveBackendId(null);
    setAvailableBackends([]);
    setCommands([]);
    setModels([]);
    setCurrentModelId(null);
    setReasoningEfforts([]);
    setCurrentReasoningEffortId(null);
    setContextUsage(null);
    setModelsLoaded(false);
    setModelChanging(false);
    gahApi
      .getManagerChatSettings()
      .then((settings) => {
        const backendId = settings.profileOverrides[profile] ?? settings.defaultBackend;
        if (!cancelled) {
          setAvailableBackends(settings.availableBackends);
          setActiveBackendId(backendId);
        }
      })
      .catch(() => { if (!cancelled) setActiveBackendId(null); });
    // Real commands from the active backend's own registry (e.g. Hermes's
    // live ACP available-commands push) -- not something GAH invents. Fetched
    // eagerly so the "/" palette has data the moment the user types it;
    // this also happens to be what warms up the backend's session.
    gahApi
      .getManagerChatCommands(profile)
      .then(({ commands }) => { if (!cancelled) setCommands(commands); })
      .catch(() => { if (!cancelled) setCommands([]); });
    // Real selectable models and reasoning efforts from the backend's own
    // ACP session state. Empty means no corresponding picker renders.
    gahApi
      .getManagerChatModels(profile)
      .then(({ models, currentModelId, reasoningEfforts: advertisedEfforts, currentReasoningEffortId: effortId, contextUsage: usage }) => {
        if (!cancelled) {
          setModels(models);
          setCurrentModelId(currentModelId);
          setReasoningEfforts(advertisedEfforts ?? []);
          setCurrentReasoningEffortId(effortId ?? null);
          setContextUsage(usage ?? null);
          setModelsLoaded(true);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setModels([]);
          setCurrentModelId(null);
          setReasoningEfforts([]);
          setCurrentReasoningEffortId(null);
          setContextUsage(null);
          setModelsLoaded(true);
        }
      });
    return () => { cancelled = true; };
  }, [profile]);

  // Session-scoped model list: when a session is active, the composer's
  // provider picker needs that session's backend models, and changes go to
  // the session record (not the profile-wide default).
  const [sessionModels, setSessionModels] = useState<ManagerModelInfo[]>([]);
  const [sessionModelsLoaded, setSessionModelsLoaded] = useState(false);
  const [sessionEfforts, setSessionEfforts] = useState<ManagerReasoningEffortInfo[]>([]);
  const [sessionSelectionChanging, setSessionSelectionChanging] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setSessionModels([]);
    setSessionModelsLoaded(false);
    setSessionEfforts([]);
    if (!activeSession || !isConnected) return;
    gahApi
      .getManagerChatModelsForBackend(profile, activeSession.backend)
      .then(({ models, reasoningEfforts: advertisedEfforts }) => {
        if (!cancelled) {
          setSessionModels(models);
          setSessionModelsLoaded(true);
          setSessionEfforts(advertisedEfforts ?? []);
        }
      })
      .catch(() => { if (!cancelled) { setSessionModels([]); setSessionModelsLoaded(true); } });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, sessionId, activeSession?.backend]);

  /** Composer provider picker, session variant: one PATCH carries the full
   *  desired selection. A backend switch resets model + effort (the picker
   *  sends null for them), matching the old select semantics — same
   *  worktree, next turn on the new provider's defaults. */
  const applySessionSelection = async (next: ProviderSelection) => {
    if (!activeSession) return;
    const backendChanged = next.backendId !== activeSession.backend;
    const modelChanged = (next.modelId ?? null) !== (activeSession.model ?? null);
    const effortChanged = (next.reasoningEffortId ?? null) !== (activeSession.reasoningEffort ?? null);
    if (!backendChanged && !modelChanged && !effortChanged) return;
    setSessionSelectionChanging(true);
    try {
      await gahApi.updateChatSession(profile, activeSession.id, {
        ...(backendChanged ? { backend: next.backendId } : {}),
        model: next.modelId,
        reasoningEffort: next.reasoningEffortId
      });
      refreshSessions(profile);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch session provider: ${err instanceof Error ? err.message : String(err)}` }]);
    } finally {
      setSessionSelectionChanging(false);
    }
  };

  const handleModelChange = async (modelId: string) => {
    setModelChanging(true);
    const requestedProfile = profile;
    try {
      await gahApi.setManagerChatModel(requestedProfile, modelId);
      const summary = await gahApi.getManagerChatModels(requestedProfile);
      if (activeProfileRef.current === requestedProfile) {
        setModels(summary.models);
        setCurrentModelId(summary.currentModelId);
        setReasoningEfforts(summary.reasoningEfforts ?? []);
        setCurrentReasoningEffortId(summary.currentReasoningEffortId ?? null);
        setContextUsage(summary.contextUsage ?? null);
      }
    } catch (err) {
      if (activeProfileRef.current === requestedProfile) {
        setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch model: ${err instanceof Error ? err.message : String(err)}` }]);
      }
    } finally {
      if (activeProfileRef.current === requestedProfile) setModelChanging(false);
    }
  };

  const handleReasoningEffortChange = async (effortId: string) => {
    if (turnBusy) return;
    setReasoningEffortChanging(true);
    const requestedProfile = profile;
    try {
      await gahApi.setManagerChatReasoningEffort(requestedProfile, effortId);
      if (activeProfileRef.current === requestedProfile) setCurrentReasoningEffortId(effortId);
    } catch (err) {
      if (activeProfileRef.current === requestedProfile) {
        setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch reasoning effort: ${err instanceof Error ? err.message : String(err)}` }]);
      }
    } finally {
      if (activeProfileRef.current === requestedProfile) setReasoningEffortChanging(false);
    }
  };

  useEffect(() => {
    // Process every not-yet-consumed message in order. React can batch
    // several websocket frames into one render, so only looking at the last
    // message would silently drop intermediate chunk fragments.
    const batch = messages.filter(({ id }) => id > processedMessageIdRef.current);
    if (batch.length === 0) return;
    processedMessageIdRef.current = batch.at(-1)!.id;

    for (const { message: last } of batch) {
      // Generic errors carry no profile/session fields. Match known steering
      // request ids before applying the conversation-scoping guard below.
      if (last.type === 'error' && steeringRequestIds.current.delete(last.requestId)) {
        setTurns((prev) => [
          ...prev.filter((turn) => turn.steeringRequestId !== last.requestId),
          { role: 'error', text: `Steering failed: ${last.error}` }
        ]);
        continue;
      }

      // Session-scoped messages must match the conversation being rendered;
      // messages without a sessionId belong to the default conversation.
      const messageProfile = 'profile' in last ? last.profile : undefined;
      const messageSession = 'sessionId' in last ? last.sessionId : undefined;
      const sameConversation =
        messageProfile === profile
        && (messageSession ?? undefined) === (sessionId ?? undefined);
      if (!sameConversation) continue;

      if (last.type === 'manager.chat.history' && last.requestId === historyRequestId.current) {
        const restored = last.turns.map(fromServerTurn);
        setTurns(restored);
        setHistoryLoaded(true);
        // #1002: reconcile live tool cards against what the log durably
        // persisted. Cards whose toolCallId is back in the restored turns are
        // dropped — the transcript now owns them; anything still live (e.g. a
        // tool event from a turn that just ended before its fold landed) stays
        // visible rather than vanishing at the turn boundary.
        const covered = new Set<string>();
        for (const t of restored) {
          if (t.role === 'tool' && t.tool) covered.add(t.tool.toolCallId);
        }
        setLiveTools((prev) => {
          const next: typeof prev = {};
          for (const [id, tool] of Object.entries(prev)) {
            if (!covered.has(id)) next[id] = tool;
          }
          return next;
        });
        // #1001: only drop the pending request once the server reports no
        // turn in flight. A mid-turn history resync (reconnect, updated push,
        // session switch) previously nulled pendingRequest while the turn was
        // still running, so the turn's terminal reply could never match again
        // and Stop left the panel busy forever.
        if (!last.streaming) setPendingRequest(null);
        lastAppliedSeqRef.current = Math.max(lastAppliedSeqRef.current, last.cursor);
        setStreaming(last.streaming?.partialText ? { turn: last.streaming.turn, text: last.streaming.partialText } : null);
        setRemoteTurnBusy(Boolean(last.streaming));
        setPermission(last.permission ? {
          permissionId: last.permission.permissionId,
          title: last.permission.title,
          options: last.permission.options,
          locations: last.permission.locations
        } : null);
        continue;
      }

      if (last.type === 'manager.chat.updated') {
        // #960: with our own turn in flight (pendingRequest set), an updated
        // for a *different* requestId belongs to another client — resyncing
        // here would prematurely resolve our busy state. When no request is
        // pending (reconnect mid-turn, foreign turn) resync unconditionally:
        // history is authoritative and reports the live streaming turn, and
        // #1001 keeps pendingRequest alive across that resync so the turn's
        // terminal reply/update still resolves cleanly.
        if (pendingRequest && last.requestId !== pendingRequest.id) continue;
        setRemoteTurnBusy(true);
        const requestId = generateRequestId();
        historyRequestId.current = requestId;
        sendMessage({
          type: 'manager.chat.historyRequest',
          requestId,
          profile,
          ...(sessionId ? { sessionId } : {})
        });
        continue;
      }

      // Live chunk tee (#959). Dedupe by seq: chunks at or below what we've
      // already applied are already part of the rendered transcript (history
      // refetch mid-turn restores the partial text plus its cursor).
      if (last.type === 'manager.chat.chunk') {
        if (last.seq <= lastAppliedSeqRef.current) continue;
        lastAppliedSeqRef.current = last.seq;
        setStreaming((prev) => (prev && prev.turn === last.turn
          ? { turn: last.turn, text: prev.text + last.text }
          : { turn: last.turn, text: last.text }));
        setRemoteTurnBusy(true);
        continue;
      }

      // Slice 3: structured tool-call activity. Latest status per toolCallId
      // wins; the cards live under the streaming bubble until the turn ends.
      if (last.type === 'manager.chat.toolCall') {
        setLiveTools((prev) => ({
          ...prev,
          [last.toolCallId]: {
            toolCallId: last.toolCallId,
            name: last.name,
            title: last.title,
            kind: last.kind,
            status: last.status,
            locations: last.locations,
            summary: last.summary
          }
        }));
        setRemoteTurnBusy(true);
        continue;
      }

      // Slice 3: a permission request blocks the turn until answered.
      if (last.type === 'manager.chat.permission') {
        setPermission({
          permissionId: last.permissionId,
          title: last.title,
          options: last.options,
          locations: last.locations
        });
        setRemoteTurnBusy(true);
        continue;
      }

      if (last.type === 'manager.chat.steered') {
        steeringRequestIds.current.delete(last.requestId);
        continue;
      }

      // WP3: preview went live for this session (manual set or
      // auto-detected dev-server port from tool output mid-turn). The
      // conversation guard above already matched profile+sessionId.
      if (last.type === 'manager.chat.preview') {
        setPreview({
          profile: last.profile,
          sessionId: last.sessionId,
          devPort: last.devPort,
          listenPort: last.listenPort,
          url: last.url
        });
        continue;
      }

      if (!pendingRequest || pendingRequest.profile !== profile) continue;
      if (!('requestId' in last) || last.requestId !== pendingRequest.id) continue;
      if (processedRequestIds.current.has(pendingRequest.id)) continue;
      processedRequestIds.current.add(pendingRequest.id);

      if (last.type === 'manager.chat.reply') {
        setStreaming(null);
        setPermission(null);
        // #1002: tool cards survive the turn boundary. Clearing them here
        // made every tool call vanish at the moment its turn completed; the
        // manager.chat.updated→history refetch below restores the persisted
        // tool turns, and rendering dedupes live vs restored by toolCallId.
        setRemoteTurnBusy(false);
        setPendingRequest(null);
        if (!last.cancelled) {
          setTurns((prev) => [...prev, {
            role: 'assistant',
            text: last.reply,
            backend: last.backend,
            model: last.model
          }]);
        }
      } else if (last.type === 'error') {
        setStreaming(null);
        setPermission(null);
        setRemoteTurnBusy(false);
        setPendingRequest(null);
        setTurns((prev) => [...prev, { role: 'error', text: last.error }]);
      }
    }
  }, [messages, pendingRequest, profile, sessionId]);

  const turnBusy = pendingRequest !== null || remoteTurnBusy || streaming !== null;
  const sendBlocked = !historyLoaded;
  // #945: the header shows the actual configured harness, and flags a
  // configured-but-unimplemented backend rather than silently falling back.
  const activeBackendInfo = availableBackends.find((b) => b.id === activeBackendId);
  const activeBackendLabel = activeBackendInfo?.displayName ?? activeBackendId ?? 'manager';
  const activeBackendUnavailable = Boolean(activeBackendInfo && !activeBackendInfo.implemented);

  const refreshSkillBinding = async () => {
    if (!skillBackend) return;
    setSkillBinding(await gahApi.getSkillBindings(profile, skillBackend, sessionId));
  };

  const handleSkillToggle = async (id: string) => {
    if (!skillBackend || !skillBinding || turnBusy) return;
    const selected = skillBinding.selectedIds.includes(id)
      ? skillBinding.selectedIds.filter((current) => current !== id)
      : [...skillBinding.selectedIds, id];
    const previous = skillBinding;
    setSkillBinding({ ...skillBinding, source: 'profile', selectedIds: selected });
    setSkillBindingChanging(true);
    try {
      await gahApi.setSkillBindings({ profile, backend: skillBackend, skillIds: selected });
      await refreshSkillBinding();
    } catch (error) {
      setSkillBinding(previous);
      setTurns((current) => [...current, { role: 'error', text: `Failed to update skills: ${error instanceof Error ? error.message : String(error)}` }]);
    } finally {
      setSkillBindingChanging(false);
    }
  };

  const handleSkillInherit = async () => {
    if (!skillBackend || turnBusy) return;
    setSkillBindingChanging(true);
    try {
      await gahApi.inheritSkillBindings(profile, skillBackend);
      await refreshSkillBinding();
    } catch (error) {
      setTurns((current) => [...current, { role: 'error', text: `Failed to restore default skills: ${error instanceof Error ? error.message : String(error)}` }]);
    } finally {
      setSkillBindingChanging(false);
    }
  };

  const handleBackendChange = async (backendId: string) => {
    // #945 AC5: never apply mid-turn -- it would misattribute the in-flight
    // reply. The picker is disabled while a turn is in flight, and this guard
    // is the belt-and-suspenders.
    if (turnBusy) return;
    setBackendChanging(true);
    const requestedProfile = profile;
    try {
      // Preserve every other profile's override (#945 AC6) -- the server
      // replaces profileOverrides wholesale, so merge over the current map.
      const settings = await gahApi.getManagerChatSettings();
      await gahApi.setManagerChatSettings({
        profileOverrides: { ...settings.profileOverrides, [requestedProfile]: backendId }
      });
      if (activeProfileRef.current === requestedProfile) {
        setActiveBackendId(backendId);
        setModelsLoaded(false);
        setReasoningEfforts([]);
        setCurrentReasoningEffortId(null);
        // Refresh the new backend's command palette + model list (the
        // settings effect only re-runs on profile change, not backend change).
        gahApi.getManagerChatCommands(requestedProfile)
          .then(({ commands }) => setCommands(commands))
          .catch(() => setCommands([]));
        gahApi.getManagerChatModels(requestedProfile)
          .then(({ models, currentModelId, reasoningEfforts: advertisedEfforts, currentReasoningEffortId: effortId, contextUsage: usage }) => {
            setModels(models);
            setCurrentModelId(currentModelId);
            setReasoningEfforts(advertisedEfforts ?? []);
            setCurrentReasoningEffortId(effortId ?? null);
            setContextUsage(usage ?? null);
            setModelsLoaded(true);
          })
          .catch(() => {
            setModels([]);
            setCurrentModelId(null);
            setReasoningEfforts([]);
            setCurrentReasoningEffortId(null);
            setContextUsage(null);
            setModelsLoaded(true);
          });
      }
    } catch (err) {
      if (activeProfileRef.current === requestedProfile) {
        setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch harness: ${err instanceof Error ? err.message : String(err)}` }]);
      }
    } finally {
      if (activeProfileRef.current === requestedProfile) setBackendChanging(false);
    }
  };

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [turns, turnBusy]);

  /** Composer provider picker, profile variant: drives the same
   *  profile-level settings the header selects do. A favorite can carry
   *  backend + model + effort, so a backend switch is followed by the
   *  pinned model/effort when the favorite specified them. */
  const applyProfileSelection = async (next: ProviderSelection) => {
    if (turnBusy) return;
    const backendChanged = next.backendId !== activeBackendId;
    if (backendChanged) {
      await handleBackendChange(next.backendId);
      if (next.modelId) await handleModelChange(next.modelId);
      if (next.reasoningEffortId) await handleReasoningEffortChange(next.reasoningEffortId);
      return;
    }
    if (next.modelId && next.modelId !== currentModelId) await handleModelChange(next.modelId);
    if (next.reasoningEffortId && next.reasoningEffortId !== currentReasoningEffortId) {
      await handleReasoningEffortChange(next.reasoningEffortId);
    }
  };

  /** Which conversation the composer's pill controls: the active session
   *  carries its own backend/model/effort; the default conversation drives
   *  the profile-level settings (same state the header selects show). */
  const composerPicker: ProviderPickerProps | null = (() => {
    if (availableBackends.length === 0) return null;
    if (activeSession) {
      if (activeSession.archivedAt !== null) return null;
      return {
        variant: 'session',
        backends: availableBackends,
        selectedBackendId: activeSession.backend,
        models: sessionModels,
        currentModelId: activeSession.model,
        reasoningEfforts: sessionEfforts,
        currentReasoningEffortId: activeSession.reasoningEffort,
        modelsLoaded: sessionModelsLoaded,
        busy: turnBusy || sessionSelectionChanging,
        onSelect: applySessionSelection
      };
    }
    if (!activeBackendId) return null;
    return {
      variant: 'profile',
      backends: availableBackends,
      selectedBackendId: activeBackendId,
      models,
      currentModelId,
      reasoningEfforts,
      currentReasoningEffortId,
      modelsLoaded,
      busy: turnBusy || backendChanging || modelChanging || reasoningEffortChanging,
      onSelect: applyProfileSelection
    };
  })();

  // "/" palette: matches commands by prefix against whatever's typed after
  // the leading slash, only while the draft is exactly a slash-command in
  // progress (not once the user has moved on to a space-separated arg or a
  // second word).
  const paletteQuery = /^\/([a-zA-Z0-9_-]*)$/.exec(draft)?.[1];
  const paletteMatches = useMemo(() => {
    if (paletteQuery === undefined) return [];
    return commands.filter((cmd) => cmd.name.startsWith(paletteQuery.toLowerCase()));
  }, [commands, paletteQuery]);

  useEffect(() => {
    setPaletteOpen(paletteQuery !== undefined && paletteMatches.length > 0);
    setPaletteIndex(0);
  }, [paletteQuery, paletteMatches.length]);

  const applyPaletteSelection = (cmd: ManagerCommandInfo) => {
    setDraft(`/${cmd.name} `);
    setPaletteOpen(false);
    inputRef.current?.focus();
  };

  const handleSend = () => {
    const text = draft.trim();
    if (!text || sendBlocked) return;
    const requestId = generateRequestId();
    setTurns((prev) => [...prev, {
      role: 'user',
      text,
      ...(turnBusy ? { steeringRequestId: requestId } : {})
    }]);
    setDraft('');
    setPaletteOpen(false);
    if (turnBusy) {
      steeringRequestIds.current.add(requestId);
      sendMessage({
        type: 'manager.chat.steer',
        requestId,
        profile,
        message: text,
        ...(sessionId ? { sessionId } : {})
      });
      return;
    }
    setPendingRequest({ id: requestId, profile });
    sendMessage({
      type: 'manager.chat.send',
      requestId,
      profile,
      message: text,
      ...(sessionId ? { sessionId } : {})
    });
  };

  const handleCancel = () => {
    if (!turnBusy) return;
    const requestId = generateRequestId();
    sendMessage({
      type: 'manager.chat.cancel',
      requestId,
      profile,
      ...(sessionId ? { sessionId } : {})
    });
  };

  /** Slice 3: answer the live permission request; the blocked turn resumes
   * (allow) or unwinds (reject) server-side. */
  const handlePermissionRespond = (optionId: string) => {
    if (!permission) return;
    const requestId = generateRequestId();
    sendMessage({
      type: 'manager.chat.permission.respond',
      requestId,
      profile,
      ...(sessionId ? { sessionId } : {}),
      permissionId: permission.permissionId,
      optionId
    });
    setPermission(null);
  };

  const handleArchiveSession = async () => {
    if (!activeSession) return;
    try {
      const archived = await gahApi.archiveChatSession(profile, activeSession.id);
      setSessions((current) => activeChatSessions(current.filter((session) => session.id !== archived.id)));
      refreshSessions(profile);
      refreshAllSessions();
      setSessionId(null);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to archive session: ${err instanceof Error ? err.message : String(err)}` }]);
    }
  };

  const handleBulkArchive = async () => {
    const ids = [...selectedSessionIds];
    if (ids.length === 0) return;
    setArchiveBusy(true);
    try {
      await gahApi.bulkArchiveChatSessions(profile, ids);
      if (sessionId && selectedSessionIds.has(sessionId)) setSessionId(null);
      setSelectedSessionIds(new Set());
      refreshSessions(profile);
      refreshAllSessions();
      await refreshStorage(profile);
    } catch (error) {
      setStorageError(error instanceof Error ? error.message : String(error));
    } finally {
      setArchiveBusy(false);
    }
  };

  const handleReclaimNow = async () => {
    setArchiveBusy(true);
    try {
      const result = await gahApi.reclaimChatSessions(profile, false);
      if (sessionId && result.sessions.some((session) => session.id === sessionId)) setSessionId(null);
      setStorage(result);
      setStorageError(result.warnings[0] ?? null);
      setSelectedSessionIds(new Set());
      refreshSessions(profile);
      refreshAllSessions();
      await refreshStorage(profile);
    } catch (error) {
      setStorageError(error instanceof Error ? error.message : String(error));
    } finally {
      setArchiveBusy(false);
    }
  };

  const [newChatOpen, setNewChatOpen] = useState(false);

  useEffect(() => {
    if (!previewOpen || !preview) return;
    setPreviewBlocked(false);
    const timer = setTimeout(() => {
      const frame = document.querySelector<HTMLIFrameElement>('iframe[title="Session preview"]');
      if (!frame) return;
      try {
        // Accessible document with no content = the load never happened.
        const doc = frame.contentDocument;
        setPreviewBlocked(Boolean(doc && !doc.body?.childNodes.length && !doc.title));
      } catch {
        // Cross-origin access throws = the frame loaded a real page.
        setPreviewBlocked(false);
      }
    }, 2000);
    return () => clearTimeout(timer);
  }, [previewOpen, preview?.url]);

  /** WP3 preview: manual port set/clear + panel toggle. */
  const applyPreviewPort = async (raw: string) => {
    if (!activeSession) return;
    const port = Number.parseInt(raw, 10);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setTurns((prev) => [...prev, { role: 'error', text: 'Preview port must be a number between 1 and 65535.' }]);
      return;
    }
    try {
      const { preview: updated } = await gahApi.setChatPreview(profile, activeSession.id, port);
      setPreview(updated);
      setPreviewPortDraft('');
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to set preview: ${err instanceof Error ? err.message : String(err)}` }]);
    }
  };

  const stopPreview = async () => {
    if (!activeSession) return;
    try {
      await gahApi.setChatPreview(profile, activeSession.id, null);
      setPreview(null);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to stop preview: ${err instanceof Error ? err.message : String(err)}` }]);
    }
  };

  /** New-chat completion: switch project if the modal picked another one,
   * then select the fresh session (its history effect clears the view).
   * The pending ref keeps the id alive through the profile-switch effect,
   * same as a cross-project picker selection. */
  const handleChatCreated = (createdProfile: string, createdSessionId: string) => {
    if (createdProfile !== profile) {
      pendingSessionRef.current = createdSessionId;
      setProfileOverride(createdProfile);
    }
    refreshSessions(createdProfile);
    // The picker's trigger label resolves against the cross-project groups,
    // so the fresh session must land there too before it can be shown.
    refreshAllSessions();
    setSessionId(createdSessionId);
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title={currentProfileInfo?.repo ? currentProfileInfo.repo.split('/').pop() ?? 'Chat' : 'Chat'}
        description={`${currentProfileInfo?.repo ?? profile} · ${
          activeSession
            ? `${activeSession.backend}${activeSession.model ? ` / ${activeSession.model}` : ''}`
            : `${activeBackendLabel}${activeBackendUnavailable ? ' (unavailable)' : ''}`
        }`}
        actions={
          <div className="flex flex-wrap items-center justify-end gap-2">
            {skillBackend && (
              <SkillPicker
                binding={skillBinding}
                busy={turnBusy || skillBindingChanging}
                onToggle={(id) => void handleSkillToggle(id)}
                onInherit={() => void handleSkillInherit()}
              />
            )}
            {/* Profile-wide pickers for the default conversation; a session
                carries its own backend/model in the session bar below. */}
            {!activeSession && availableBackends.length > 0 && (
              <select
                value={activeBackendId ?? ''}
                onChange={(e) => handleBackendChange(e.target.value)}
                disabled={backendChanging || turnBusy}
                title={turnBusy ? 'Switching harness is disabled while a turn is in flight' : 'Harness / backend'}
                className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary max-w-[170px]"
                aria-label="Harness / backend"
              >
                {availableBackends.map((b) => (
                  <option key={b.id} value={b.id}>
                    {b.displayName}{b.implemented ? '' : ' (unavailable)'}
                  </option>
                ))}
              </select>
            )}
            {!activeSession && models.length > 0 && (
              <select
                value={currentModelId ?? ''}
                onChange={(e) => handleModelChange(e.target.value)}
                disabled={modelChanging}
                className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary max-w-[220px]"
                aria-label="Model"
              >
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
            )}
            {!activeSession && reasoningEfforts.length > 0 && (
              <select
                value={currentReasoningEffortId ?? ''}
                onChange={(e) => handleReasoningEffortChange(e.target.value)}
                disabled={reasoningEffortChanging || modelChanging || backendChanging || turnBusy}
                title={turnBusy ? 'Switching reasoning effort is disabled while a turn is in flight' : 'Reasoning effort'}
                className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary max-w-[170px]"
                aria-label="Reasoning effort"
              >
                {reasoningEfforts.map((effort) => (
                  <option key={effort.id} value={effort.id} title={effort.description}>
                    {effort.name}
                  </option>
                ))}
              </select>
            )}
            {/* #865: context-window occupancy from the backend's own
                usage_update push (Hermes today). Hidden entirely for a
                backend that doesn't advertise it, the same empty-state
                pattern as the model picker just below. */}
            {!activeSession && contextUsage && (
              <span
                aria-label="Context usage"
                className="rounded-md border border-subtle bg-raised px-2 py-1.5 text-[11px] tabular-nums text-muted"
                title={`${contextUsage.used.toLocaleString()} / ${contextUsage.size.toLocaleString()} tokens in context`}
              >
                {Math.round((contextUsage.used / contextUsage.size) * 100)}% context
              </span>
            )}
            {/* This provider doesn't expose a model picker over its protocol
                (e.g. Hermes over ACP): say so instead of silently hiding the
                control. Codex / Claude / OpenCode expose live lists. */}
            {!activeSession && modelsLoaded && models.length === 0 && activeBackendId && (
              <span
                className="rounded-md border border-subtle bg-raised px-2 py-1.5 text-[11px] text-muted"
                title="This provider uses its configured default model and doesn't expose a picker. Switch to a provider that advertises models or use its own model command in chat."
              >
                Default model · {activeBackendLabel}
              </span>
            )}
          </div>
        }
      />

      {/* Git strip: compact git state for the active session's project */}
      {currentProfileInfo && (
        <div className="card-padded">
          <GitStrip profile={profile} status={gitStatus} issues={gitIssues} prs={gitPrs} onRefresh={loadGitData} />
        </div>
      )}

      {/* WP2 session bar: one conversation per worktree. Default = the
          profile's shared conversation; sessions run in isolated worktrees
          (branch survives archive; a reclaimed worktree rematerializes on
          resume). "New chat" is the T3 flow: project → node → provider. */}
      <div className="flex items-center gap-2 flex-wrap">
        <GitBranch size={14} className="text-muted shrink-0" aria-hidden="true" />
        <ChatSessionPicker
          groups={allSessions}
          selectedProfile={profile}
          selectedSessionId={sessionId}
          onSelect={handlePickerSelect}
        />
        {(sessionsError || allSessionsError) && (
          <button
            type="button"
            onClick={() => { refreshSessions(profile); refreshAllSessions(); }}
            className="rounded-md border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-[11px] text-amber-300 hover:bg-amber-500/20"
            title="Session list failed to load — showing the last known sessions. Click to retry."
          >
            Sessions · retry
          </button>
        )}
        <button
          onClick={() => setNewChatOpen(true)}
          disabled={!isConnected}
          className="btn-primary text-xs inline-flex items-center gap-1"
          title="New chat: choose project, node, and provider/model — starts in a fresh worktree"
        >
          <Plus size={13} aria-hidden="true" /> New chat
        </button>
        <button
          type="button"
          onClick={() => setStorageOpen((open) => !open)}
          className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-secondary hover:bg-white/5 inline-flex items-center gap-1"
          aria-expanded={storageOpen}
          title="Inspect per-session storage and preview safe reclaim"
        >
          <HardDrive size={13} aria-hidden="true" /> Storage
        </button>
        {activeSession && activeSession.archivedAt === null && (
          <>
            {/* Session provider/model/effort switching moved into the
                composer's provider pill (t3-style picker with favorites). */}
            <button
              onClick={() => setPreviewOpen((v) => !v)}
              disabled={turnBusy && !preview}
              className={`inline-flex items-center gap-1 rounded-md border px-2 py-1.5 text-xs disabled:opacity-50 ${
                preview
                  ? 'border-accent/40 bg-accent/15 text-primary'
                  : 'border-subtle bg-raised text-secondary hover:bg-white/5'
              }`}
              title={preview
                ? `Preview live: dev server on :${preview.devPort} → ${preview.url}`
                : 'Open the preview panel — set the dev-server port, or let GAH auto-detect it from tool output'}
            >
              <MonitorPlay size={13} aria-hidden="true" />
              Preview{preview ? ` :${preview.devPort}` : ''}
            </button>
            <button
              onClick={handleArchiveSession}
              disabled={turnBusy}
              className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-secondary hover:bg-white/5 disabled:opacity-50 inline-flex items-center gap-1"
              title="Archive this session (dirty work is saved as a patch; the branch survives)"
            >
              <Archive size={13} aria-hidden="true" /> Archive
            </button>
          </>
        )}
        {activeSession?.outcome !== 'live' && activeSession && (
          <span className="text-[10px] text-muted">
            {activeSession.outcome === 'settled' ? `settled · ${activeSession.settledReason ?? 'delivered'}` : 'archived'} — read only
          </span>
        )}
      </div>

      {storageOpen && (() => {
        const profileStorage = storage?.profiles[0];
        const storageBySession = new Map(profileStorage?.sessions.map((item) => [item.sessionId, item]) ?? []);
        const liveSessions = sessions.filter((session) => session.outcome === 'live');
        const idleIds = liveSessions.filter((session) => storageBySession.get(session.id)?.idle).map((session) => session.id);
        return (
          <section className="card-padded space-y-3" aria-label="Chat storage">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 className="text-sm font-semibold text-primary">Chat storage · {profile}</h2>
                <p className="text-[11px] text-muted">
                  {profileStorage
                    ? `${formatBytes(profileStorage.worktreeBytes)} in worktrees · ${formatBytes(profileStorage.projectedReclaimBytes)} projected reclaim · idle after ${profileStorage.idleDays} days`
                    : 'Calculating worktree usage and dry-run projection…'}
                </p>
              </div>
              <div className="flex flex-wrap gap-1.5">
                <button
                  type="button"
                  onClick={() => setSelectedSessionIds(new Set(idleIds))}
                  disabled={idleIds.length === 0 || archiveBusy}
                  className="btn-secondary text-xs"
                >
                  Select idle ({idleIds.length})
                </button>
                <button
                  type="button"
                  onClick={() => void handleBulkArchive()}
                  disabled={selectedSessionIds.size === 0 || archiveBusy}
                  className="btn-secondary text-xs inline-flex items-center gap-1"
                >
                  <Archive size={12} aria-hidden="true" /> Archive selected ({selectedSessionIds.size})
                </button>
                <button
                  type="button"
                  onClick={() => void handleReclaimNow()}
                  disabled={!storage || storage.candidates.length === 0 || archiveBusy}
                  className="btn-primary text-xs"
                  title="Apply this dry-run plan through the patch-preserving archive path"
                >
                  Reclaim now ({storage?.candidates.length ?? 0})
                </button>
                <button
                  type="button"
                  onClick={() => void refreshStorage(profile)}
                  disabled={storageLoading || archiveBusy}
                  className="btn-secondary text-xs"
                  aria-label="Refresh storage dry run"
                >
                  <RefreshCw size={12} className={storageLoading ? 'animate-spin' : ''} aria-hidden="true" />
                </button>
              </div>
            </div>
            {storageError && <p className="text-xs text-red-400" role="alert">{storageError}</p>}
            {storage?.warnings.map((warning) => <p key={warning} className="text-xs text-amber-300">{warning}</p>)}
            <div className="divide-y divide-subtle rounded-md border border-subtle">
              {liveSessions.map((session) => {
                const item = storageBySession.get(session.id);
                const candidate = storage?.candidates.find((entry) => entry.sessionId === session.id);
                return (
                  <label key={session.id} className="flex cursor-pointer items-center gap-3 px-3 py-2 text-xs hover:bg-white/5">
                    <input
                      type="checkbox"
                      checked={selectedSessionIds.has(session.id)}
                      onChange={(event) => setSelectedSessionIds((current) => {
                        const next = new Set(current);
                        if (event.target.checked) next.add(session.id); else next.delete(session.id);
                        return next;
                      })}
                      aria-label={`Select ${session.title ?? session.branch}`}
                    />
                    <span className="min-w-0 flex-1 truncate text-primary">{session.title ?? session.branch}</span>
                    {item?.idle && <span className="text-amber-300">idle</span>}
                    {candidate && <span className="text-accent">{candidate.outcome === 'settled' ? `settle · ${candidate.reason}` : 'archive · idle'}</span>}
                    <span className="font-mono text-muted">{formatBytes(item?.worktreeBytes ?? 0)}</span>
                    {(item?.projectedReclaimBytes ?? 0) > 0 && (
                      <span className="font-mono text-accent">→ {formatBytes(item?.projectedReclaimBytes ?? 0)}</span>
                    )}
                  </label>
                );
              })}
              {liveSessions.length === 0 && <p className="px-3 py-4 text-xs text-muted">No live chat sessions.</p>}
            </div>
            <p className="text-[10px] text-muted">Dry run only until you choose Reclaim now or Archive selected. Dirty work is saved as a patch; every branch survives.</p>
          </section>
        );
      })()}

      <NewChatModal
        open={newChatOpen}
        currentProfile={profile}
        profiles={availableProfiles}
        backends={availableBackends}
        onClose={() => setNewChatOpen(false)}
        onCreated={handleChatCreated}
      />

      <div className={`grid min-w-0 gap-4 ${previewOpen && activeSession ? 'xl:grid-cols-[14rem_minmax(0,1fr)_minmax(0,26rem)]' : 'xl:grid-cols-[14rem_minmax(0,1fr)]'}`}>
        <ProjectRail
          currentProfile={profile}
          profiles={availableProfiles}
          onSelect={setProfileOverride}
          onProjectAdded={(project) => {
            setAvailableProfiles((profiles) => [...profiles.filter((profile) => profile.name !== project.name), project]);
          }}
        />

      {/* WP3 preview panel: the session's dev server through the node's
          dedicated preview port. Auto-detect lights it up mid-turn; the
          port can also be set manually. */}
      {previewOpen && activeSession && (
        <div className="card-padded flex min-w-0 flex-col h-[65vh] order-3 xl:order-none">
          <div className="flex items-center justify-between gap-2 pb-2">
            <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
              <MonitorPlay size={13} aria-hidden="true" /> Preview
            </h3>
            <div className="flex items-center gap-1">
              {preview && safePreviewUrl(preview.url) !== null && (
                <a
                  href={safePreviewUrl(preview.url) ?? undefined}
                  target="_blank"
                  rel="noreferrer"
                  className="rounded p-1 text-muted hover:bg-white/5 hover:text-primary"
                  title="Open in a new tab"
                >
                  <ExternalLink size={13} aria-hidden="true" />
                </a>
              )}
              <button
                onClick={() => setPreviewOpen(false)}
                className="rounded p-1 text-muted hover:bg-white/5 hover:text-primary"
                aria-label="Close preview"
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>
          </div>
          {preview && safePreviewUrl(preview.url) === null ? (
            <p className="text-xs text-red-400 px-4 py-6">
              Preview URL failed validation and was blocked.
            </p>
          ) : preview ? (
            <>
              <p className="text-[11px] text-muted pb-2">
                dev server <span className="font-mono">:{preview.devPort}</span> → <span className="font-mono">{safePreviewUrl(preview.url)}</span>
              </p>
              <div className="relative flex-1 min-h-0">
                <iframe
                  src={safePreviewUrl(preview.url) ?? undefined}
                  title="Session preview"
                  className="h-full min-h-0 w-full rounded-md border border-subtle bg-white"
                  sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
                />
                {previewBlocked && (
                  <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 rounded-md bg-card/95 px-4 text-center">
                    <p className="text-xs leading-relaxed text-secondary">
                      The browser blocked the embedded preview (HTTPS page, HTTP preview).
                      Opening it in a new tab works — the preview URL is served over the
                      tailnet.
                    </p>
                    <a
                      href={safePreviewUrl(preview.url) ?? '#'}
                      target="_blank"
                      rel="noreferrer"
                      className="btn-primary text-xs"
                    >
                      <ExternalLink size={13} aria-hidden="true" /> Open preview
                    </a>
                  </div>
                )}
              </div>
              <button
                onClick={stopPreview}
                className="btn-secondary text-xs mt-2 self-start"
                title="Stop proxying this port"
              >
                Stop preview
              </button>
            </>
          ) : (
            <div className="flex-1 flex flex-col items-center justify-center text-center text-muted gap-3 px-4">
              <MonitorPlay size={24} className="opacity-50" aria-hidden="true" />
              <p className="text-xs leading-relaxed">
                No preview yet. Ask the agent to start a dev server in this session
                (e.g. <span className="font-mono">npm run dev</span>) — GAH auto-detects
                the port from the tool output. Or set it manually:
              </p>
              <form
                className="flex gap-1.5 w-full max-w-[14rem]"
                onSubmit={(e) => { e.preventDefault(); void applyPreviewPort(previewPortDraft); }}
              >
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={previewPortDraft}
                  onChange={(e) => setPreviewPortDraft(e.target.value)}
                  placeholder="e.g. 5173"
                  className="min-w-0 flex-1 rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary"
                  aria-label="Dev server port"
                />
                <button type="submit" className="btn-primary text-xs" disabled={!previewPortDraft}>
                  Set
                </button>
              </form>
            </div>
          )}
        </div>
      )}

      <div className="card-padded flex min-w-0 flex-col h-[65vh]">
        <div className="flex-1 overflow-y-auto space-y-3 pr-1">
          {!historyLoaded && turns.length === 0 && (
            <div className="h-full flex flex-col items-center justify-center text-center text-muted gap-2">
              <MessageSquare size={24} className="opacity-50 animate-pulse" aria-hidden="true" />
              <p className="text-sm">Loading conversation…</p>
            </div>
          )}
          {historyLoaded && turns.length === 0 && (
            <div className="h-full flex flex-col items-center justify-center text-center text-muted gap-2">
              <MessageSquare size={24} className="opacity-50" aria-hidden="true" />
              <p className="text-sm">Ask the manager about this project's status, blockers, or next actions. Type "/" for commands.</p>
              <p className="text-xs text-muted">Context is shared across backends — switching models keeps this session's memory.</p>
            </div>
          )}
          {turns.filter((turn) => !turn.tool || !liveTools[turn.tool.toolCallId]).map((turn, i) => (
            <div key={i} className={`flex flex-col ${turn.role === 'user' ? 'items-end' : 'items-start'}`}>
              {turn.role === 'tool' && turn.tool ? (
                <ToolCallCard tool={turn.tool} />
              ) : (
                <div
                  className={`min-w-0 max-w-[80%] rounded-lg px-3 py-2 text-sm break-words ${
                    turn.role === 'user'
                      ? 'bg-accent text-white'
                      : turn.role === 'error'
                        ? 'bg-red-500/10 text-red-400 border border-red-500/30'
                        : turn.role === 'system'
                          ? 'bg-transparent text-muted italic text-xs px-0'
                          : 'bg-raised text-primary border border-subtle'
                  }`}
                >
                  <MarkdownMessage text={turn.text} />
                </div>
              )}
              {turn.role === 'assistant' && turn.backend && (
                <span className="mt-0.5 px-1.5 py-0.5 rounded bg-raised border border-subtle text-[10px] text-muted font-mono">
                  {turn.backend}
                  {turn.model ? ` / ${turn.model}` : ''}
                </span>
              )}
            </div>
          ))}
          {/* Slice 3: live tool activity for the in-flight turn. */}
          {Object.entries(liveTools).map(([id, tool]) => (
            <ToolCallCard key={id} tool={tool} />
          ))}
          {/* Slice 3: permission prompt -- the turn is blocked until one of
              these is clicked (or it times out / is cancelled server-side). */}
          {permission && (
            <div className="w-full max-w-[80%] rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2.5 space-y-2" role="alertdialog" aria-label="Permission request">
              <div className="flex items-start gap-2">
                <ShieldAlert size={15} className="text-amber-400 shrink-0 mt-0.5" aria-hidden="true" />
                <div className="min-w-0">
                  <p className="text-sm text-primary font-medium break-words">{permission.title}</p>
                  {permission.locations.length > 0 && (
                    <p className="text-[11px] text-muted truncate">{permission.locations.join(', ')}</p>
                  )}
                </div>
              </div>
              <div className="flex flex-wrap gap-1.5 justify-end">
                {permission.options.map((opt) => (
                  <button
                    key={opt.optionId}
                    type="button"
                    onClick={() => handlePermissionRespond(opt.optionId)}
                    className={
                      opt.kind.startsWith('allow')
                        ? 'btn-primary text-xs'
                        : 'bg-raised border border-subtle rounded-md px-2.5 py-1 text-xs text-secondary hover:bg-white/5'
                    }
                  >
                    {opt.name}
                  </button>
                ))}
              </div>
            </div>
          )}
          {streaming && (
            <div className="flex justify-start">
              <div className="min-w-0 max-w-[80%] rounded-lg px-3 py-2 text-sm break-words bg-raised text-primary border border-subtle">
                <MarkdownMessage text={streaming.text} />
              </div>
            </div>
          )}
          {turnBusy && !streaming && (
            <div className="flex justify-start">
              <div className="max-w-[80%] rounded-lg px-3 py-2 text-sm bg-raised text-muted border border-subtle animate-pulse">
                Thinking…
              </div>
            </div>
          )}
          <div ref={scrollAnchorRef} />
        </div>

        <div className="relative flex items-end gap-2 pt-3 mt-3 border-t border-subtle">
          {composerPicker && (
            <ProviderPicker {...composerPicker} />
          )}
          {paletteOpen && (
            <div className="absolute bottom-full left-0 mb-1 w-full max-w-sm bg-card border border-subtle rounded-md shadow-lg overflow-hidden z-10">
              {paletteMatches.map((cmd, i) => (
                <button
                  key={cmd.name}
                  onClick={() => applyPaletteSelection(cmd)}
                  onMouseEnter={() => setPaletteIndex(i)}
                  className={`w-full text-left px-3 py-1.5 text-xs flex items-baseline gap-2 ${
                    i === paletteIndex ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'
                  }`}
                >
                  <span className="font-mono shrink-0">/{cmd.name}</span>
                  <span className="truncate opacity-80">{cmd.description}</span>
                </button>
              ))}
            </div>
          )}
          <textarea
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (paletteOpen) {
                if (e.key === 'ArrowDown') {
                  e.preventDefault();
                  setPaletteIndex((i) => (i + 1) % paletteMatches.length);
                  return;
                }
                if (e.key === 'ArrowUp') {
                  e.preventDefault();
                  setPaletteIndex((i) => (i - 1 + paletteMatches.length) % paletteMatches.length);
                  return;
                }
                if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
                  e.preventDefault();
                  applyPaletteSelection(paletteMatches[paletteIndex]);
                  return;
                }
                if (e.key === 'Escape') {
                  setPaletteOpen(false);
                  return;
                }
              }
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder={isConnected ? 'Message the manager… (try "/")' : 'Not connected to server'}
            disabled={!isConnected}
            rows={2}
            className="flex-1 bg-raised border border-subtle rounded-md px-3 py-2 text-sm text-primary resize-none disabled:opacity-50"
          />
          <button
            onClick={handleSend}
            disabled={!isConnected || !draft.trim() || sendBlocked}
            className="btn-primary h-fit"
            aria-label="Send"
            title={turnBusy ? 'Steer this turn' : undefined}
          >
            <Send size={14} aria-hidden="true" />
          </button>
          {turnBusy && (
            <button
              onClick={handleCancel}
              className="btn-primary h-fit"
              aria-label="Stop"
              title="Stop this turn"
            >
              <Square size={14} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
      </div>
    </div>
  );
}
