import { useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { Send, Square, MessageSquare, GitBranch, Plus, Archive, Wrench, ShieldAlert, MonitorPlay, X, ExternalLink } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { NewChatModal } from '../components/NewChatModal.js';
import { ProjectRail } from '../components/ProjectRail.js';
import { gahApi } from '../api/client.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type { ManagerChatTurn, ManagerCommandInfo, ManagerModelInfo, ProfileSummary, ManagerBackendInfo, ChatSessionSummary, ChatPreviewInfo } from '@git-agent-harness/contracts';

interface ChatTurn {
  role: 'user' | 'assistant' | 'system' | 'error' | 'tool';
  text: string;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
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
  const processedMessagesRef = useRef(0);
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
  /** True once the model list fetch completed (an empty list is "this
   * backend exposes no picker", not "still loading"). */
  const [modelsLoaded, setModelsLoaded] = useState(false);
  const [modelChanging, setModelChanging] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const processedRequestIds = useRef(new Set<string>());
  const historyRequestId = useRef<string | null>(null);
  const activeProfileRef = useRef(profile);
  activeProfileRef.current = profile;
  const scrollAnchorRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  /** WP2 sessions: null = the profile's default conversation; otherwise a
   * session bound to its own worktree. */
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const activeSession = useMemo(
    () => sessions.find((s) => s.id === sessionId) ?? null,
    [sessions, sessionId]
  );

  useEffect(() => {
    gahApi.getProfiles().then(setAvailableProfiles).catch(() => {});
  }, []);

  const refreshSessions = (forProfile: string) => {
    gahApi
      .getChatSessions(forProfile)
      .then(({ sessions }) => {
        if (activeProfileRef.current === forProfile) setSessions(sessions);
      })
      .catch(() => { if (activeProfileRef.current === forProfile) setSessions([]); });
  };

  useEffect(() => {
    refreshSessions(profile);
    return () => { setSessions([]); setSessionId(null); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile]);

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
    if (sessionId && sessionId !== 'default') {
      gahApi
        .getChatPreview(profile, sessionId)
        .then(({ preview }) => { if (activeProfileRef.current === profile) setPreview(preview); })
        .catch(() => {});
    }
    lastAppliedSeqRef.current = 0;
    processedMessagesRef.current = 0;
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
    // Real selectable models from the backend's own ACP session state --
    // empty for backends that don't expose this (e.g. Claude's bridge
    // today), in which case no picker renders at all.
    gahApi
      .getManagerChatModels(profile)
      .then(({ models, currentModelId }) => {
        if (!cancelled) {
          setModels(models);
          setCurrentModelId(currentModelId);
          setModelsLoaded(true);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setModels([]);
          setCurrentModelId(null);
          setModelsLoaded(true);
        }
      });
    return () => { cancelled = true; };
  }, [profile]);

  // Session-scoped model list: when a session is active, the picker needs
  // that session's backend models, and changes go to the session record
  // (not the profile-wide default).
  const [sessionModels, setSessionModels] = useState<ManagerModelInfo[]>([]);
  const [sessionModelsLoaded, setSessionModelsLoaded] = useState(false);
  const [sessionModelChanging, setSessionModelChanging] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setSessionModels([]);
    setSessionModelsLoaded(false);
    if (!activeSession || !isConnected) return;
    gahApi
      .getManagerChatModelsForBackend(profile, activeSession.backend)
      .then(({ models }) => { if (!cancelled) { setSessionModels(models); setSessionModelsLoaded(true); } })
      .catch(() => { if (!cancelled) { setSessionModels([]); setSessionModelsLoaded(true); } });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, sessionId, activeSession?.backend]);

  const handleSessionModelChange = async (modelId: string) => {
    if (!activeSession) return;
    setSessionModelChanging(true);
    try {
      await gahApi.updateChatSession(profile, activeSession.id, { model: modelId });
      refreshSessions(profile);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch session model: ${err instanceof Error ? err.message : String(err)}` }]);
    } finally {
      setSessionModelChanging(false);
    }
  };

  const handleSessionBackendChange = async (backendId: string) => {
    if (!activeSession || backendId === activeSession.backend) return;
    // Backend switch: same worktree, new provider. The model reset matches
    // the new backend's default (model ids are backend-specific).
    try {
      await gahApi.updateChatSession(profile, activeSession.id, { backend: backendId, model: null });
      refreshSessions(profile);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch session backend: ${err instanceof Error ? err.message : String(err)}` }]);
    }
  };

  const handleModelChange = async (modelId: string) => {
    setModelChanging(true);
    const requestedProfile = profile;
    try {
      await gahApi.setManagerChatModel(requestedProfile, modelId);
      if (activeProfileRef.current === requestedProfile) setCurrentModelId(modelId);
    } catch (err) {
      if (activeProfileRef.current === requestedProfile) {
        setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch model: ${err instanceof Error ? err.message : String(err)}` }]);
      }
    } finally {
      if (activeProfileRef.current === requestedProfile) setModelChanging(false);
    }
  };

  useEffect(() => {
    // Process every not-yet-consumed message in order. React can batch
    // several websocket frames into one render, so only looking at the last
    // message would silently drop intermediate chunk fragments.
    const batch = messages.slice(processedMessagesRef.current);
    if (batch.length === 0) return;
    processedMessagesRef.current = messages.length;

    for (const last of batch) {
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
        setPendingRequest(null);
        lastAppliedSeqRef.current = Math.max(lastAppliedSeqRef.current, last.cursor);
        setStreaming(last.streaming?.partialText ? { turn: last.streaming.turn, text: last.streaming.partialText } : null);
        setRemoteTurnBusy(Boolean(last.streaming));
        continue;
      }

      if (last.type === 'manager.chat.updated') {
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
        setLiveTools({});
        setRemoteTurnBusy(false);
        setPendingRequest(null);
        setTurns((prev) => [...prev, {
          role: 'assistant',
          text: last.reply,
          backend: last.backend,
          model: last.model
        }]);
      } else if (last.type === 'error') {
        setStreaming(null);
        setPermission(null);
        setLiveTools({});
        setRemoteTurnBusy(false);
        setPendingRequest(null);
        setTurns((prev) => [...prev, { role: 'error', text: last.error }]);
      }
    }
  }, [messages, pendingRequest, profile, sessionId]);

  const turnBusy = pendingRequest !== null || remoteTurnBusy || streaming !== null;
  const sendBlocked = !historyLoaded || turnBusy;
  // #945: the header shows the actual configured harness, and flags a
  // configured-but-unimplemented backend rather than silently falling back.
  const activeBackendInfo = availableBackends.find((b) => b.id === activeBackendId);
  const activeBackendLabel = activeBackendInfo?.displayName ?? activeBackendId ?? 'manager';
  const activeBackendUnavailable = Boolean(activeBackendInfo && !activeBackendInfo.implemented);

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
        // Refresh the new backend's command palette + model list (the
        // settings effect only re-runs on profile change, not backend change).
        gahApi.getManagerChatCommands(requestedProfile)
          .then(({ commands }) => setCommands(commands))
          .catch(() => setCommands([]));
        gahApi.getManagerChatModels(requestedProfile)
          .then(({ models, currentModelId }) => {
            setModels(models);
            setCurrentModelId(currentModelId);
          })
          .catch(() => { setModels([]); setCurrentModelId(null); });
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
    setTurns((prev) => [...prev, { role: 'user', text }]);
    setDraft('');
    setPaletteOpen(false);
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
      await gahApi.archiveChatSession(profile, activeSession.id);
      refreshSessions(profile);
      setSessionId(null);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to archive session: ${err instanceof Error ? err.message : String(err)}` }]);
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
   * then select the fresh session (its history effect clears the view). */
  const handleChatCreated = (createdProfile: string, createdSessionId: string) => {
    if (createdProfile !== profile) setProfileOverride(createdProfile);
    refreshSessions(createdProfile);
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
          <div className="flex items-center gap-2">
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
            {/* This provider doesn't expose a model picker over its protocol
                (e.g. Hermes over ACP): say so instead of silently hiding the
                control. Codex / Claude / OpenCode expose live lists. */}
            {!activeSession && modelsLoaded && models.length === 0 && activeBackendId && (
              <span
                className="rounded-md border border-subtle bg-raised px-2 py-1.5 text-[11px] text-muted"
                title="This provider uses its configured default model and doesn't expose a picker. Switch provider (Codex, Claude, OpenCode expose live model lists) or use its own model command in chat."
              >
                Default model · {activeBackendLabel}
              </span>
            )}
          </div>
        }
      />

      {/* WP2 session bar: one conversation per worktree. Default = the
          profile's shared conversation; sessions run in isolated worktrees
          (branch survives archive; a reclaimed worktree rematerializes on
          resume). "New chat" is the T3 flow: project → node → provider. */}
      <div className="flex items-center gap-2 flex-wrap">
        <GitBranch size={14} className="text-muted shrink-0" aria-hidden="true" />
        <select
          value={sessionId ?? ''}
          onChange={(e) => setSessionId(e.target.value || null)}
          className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary min-w-[12rem]"
          aria-label="Chat session"
        >
          <option value="">Default conversation</option>
          {sessions.filter((s) => s.archivedAt === null).map((s) => (
            <option key={s.id} value={s.id}>
              {s.title ?? s.branch}
            </option>
          ))}
        </select>
        <button
          onClick={() => setNewChatOpen(true)}
          disabled={!isConnected}
          className="btn-primary text-xs inline-flex items-center gap-1"
          title="New chat: choose project, node, and provider/model — starts in a fresh worktree"
        >
          <Plus size={13} aria-hidden="true" /> New chat
        </button>
        {activeSession && activeSession.archivedAt === null && (
          <>
            {/* Session backend/model quick-switch: the "interchangeable
                worktree" control. Same worktree, next turn on the new
                provider/model. */}
            <select
              value={activeSession.backend}
              onChange={(e) => handleSessionBackendChange(e.target.value)}
              disabled={turnBusy || sessionModelChanging}
              className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary"
              aria-label="Session provider"
              title="Switch provider for this session — same worktree, same branch"
            >
              {availableBackends.map((b) => (
                <option key={b.id} value={b.id} disabled={!b.implemented}>
                  {b.displayName}{b.implemented ? '' : ' (unavailable)'}
                </option>
              ))}
            </select>
            {sessionModels.length > 0 && (
              <select
                value={activeSession.model ?? ''}
                onChange={(e) => handleSessionModelChange(e.target.value)}
                disabled={turnBusy || sessionModelChanging}
                className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary max-w-[200px]"
                aria-label="Session model"
              >
                {!activeSession.model && <option value="">Default model</option>}
                {sessionModels.map((m) => (
                  <option key={m.id} value={m.id}>{m.name}</option>
                ))}
              </select>
            )}
            {sessionModelsLoaded && sessionModels.length === 0 && (
              <span
                className="rounded-md border border-subtle bg-raised px-2 py-1.5 text-[11px] text-muted"
                title="This provider uses its configured default model and doesn't expose a picker. Switch the session provider to pick a model."
              >
                Default model
              </span>
            )}
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
        {activeSession?.archivedAt != null && (
          <span className="text-[10px] text-muted">archived — read only</span>
        )}
      </div>

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
          {turns.map((turn, i) => (
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
          {turnBusy ? (
            <button
              onClick={handleCancel}
              className="btn-primary h-fit"
              aria-label="Stop"
              title="Stop this turn"
            >
              <Square size={14} aria-hidden="true" />
            </button>
          ) : (
            <button
              onClick={handleSend}
              disabled={!isConnected || !draft.trim() || sendBlocked}
              className="btn-primary h-fit"
              aria-label="Send"
            >
              <Send size={14} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
      </div>
    </div>
  );
}
