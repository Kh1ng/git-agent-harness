import { useEffect, useMemo, useRef, useState } from 'react';
import { Send, Square, MessageSquare, GitBranch, Plus, Archive } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { ProjectRail } from '../components/ProjectRail.js';
import { gahApi } from '../api/client.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type { ManagerChatTurn, ManagerCommandInfo, ManagerModelInfo, ProfileSummary, ManagerBackendInfo, ChatSessionSummary } from '@git-agent-harness/contracts';

interface ChatTurn {
  role: 'user' | 'assistant' | 'system' | 'error';
  text: string;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
}

interface PendingRequest {
  id: string;
  profile: string;
}

interface StreamingTurn {
  turn: number;
  text: string;
}

function fromServerTurn(turn: ManagerChatTurn): ChatTurn {
  return {
    role: turn.role === 'assistant' ? 'assistant' : turn.role,
    text: turn.text,
    backend: turn.backend,
    model: turn.model
  };
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
    // live ACP available-commands push) -- not a list GAH invents. Fetched
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
        }
      })
      .catch(() => {
        if (!cancelled) {
          setModels([]);
          setCurrentModelId(null);
        }
      });
    return () => { cancelled = true; };
  }, [profile]);

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

      if (!pendingRequest || pendingRequest.profile !== profile) continue;
      if (!('requestId' in last) || last.requestId !== pendingRequest.id) continue;
      if (processedRequestIds.current.has(pendingRequest.id)) continue;
      processedRequestIds.current.add(pendingRequest.id);

      if (last.type === 'manager.chat.reply') {
        setStreaming(null);
        setTurns((prev) => [...prev, {
          role: 'assistant',
          text: last.reply,
          backend: last.backend,
          model: last.model
        }]);
      } else if (last.type === 'error') {
        setStreaming(null);
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

  const handleNewSession = async () => {
    try {
      // No explicit backend: the session captures the profile's current
      // default at create time (server-side resolution).
      const session = await gahApi.createChatSession(profile);
      refreshSessions(profile);
      setSessionId(session.id);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to create session: ${err instanceof Error ? err.message : String(err)}` }]);
    }
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

  return (
    <div className="space-y-6">
      <PageHeader
        title={currentProfileInfo?.repo ? currentProfileInfo.repo.split('/').pop() ?? 'Chat' : 'Chat'}
        description={`${currentProfileInfo?.repo ?? profile} · ${activeBackendLabel}${activeBackendUnavailable ? ' (unavailable)' : ''}`}
        actions={
          <div className="flex items-center gap-2">
            {availableBackends.length > 0 && (
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
            {models.length > 0 && (
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
          </div>
        }
      />

      {/* WP2 session bar: one conversation per worktree. Default = the
          profile's shared conversation; sessions run in isolated worktrees
          (branch survives archive; a reclaimed worktree rematerializes on
          resume). */}
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
          onClick={handleNewSession}
          disabled={!isConnected || turnBusy}
          className="btn-primary text-xs inline-flex items-center gap-1"
          title="Start a session in a fresh worktree"
        >
          <Plus size={13} aria-hidden="true" /> Session
        </button>
        {sessionId && (
          <>
            <button
              onClick={handleArchiveSession}
              disabled={turnBusy}
              className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-secondary hover:bg-white/5 disabled:opacity-50 inline-flex items-center gap-1"
              title="Archive this session (dirty work is saved as a patch; the branch survives)"
            >
              <Archive size={13} aria-hidden="true" /> Archive
            </button>
            {sessions.some((s) => s.id === sessionId && s.archivedAt !== null) && (
              <span className="text-[10px] text-muted">archived — read only</span>
            )}
          </>
        )}
      </div>

      <div className="grid min-w-0 gap-4 xl:grid-cols-[14rem_minmax(0,1fr)]">
        <ProjectRail
          currentProfile={profile}
          profiles={availableProfiles}
          onSelect={setProfileOverride}
          onProjectAdded={(project) => {
            setAvailableProfiles((profiles) => [...profiles.filter((profile) => profile.name !== project.name), project]);
          }}
        />

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
              <div
                className={`max-w-[80%] rounded-lg px-3 py-2 text-sm whitespace-pre-wrap break-words ${
                  turn.role === 'user'
                    ? 'bg-accent text-white'
                    : turn.role === 'error'
                      ? 'bg-red-500/10 text-red-400 border border-red-500/30'
                      : turn.role === 'system'
                        ? 'bg-transparent text-muted italic text-xs px-0'
                        : 'bg-raised text-primary border border-subtle'
                }`}
              >
                {turn.text}
              </div>
              {turn.role === 'assistant' && turn.backend && (
                <span className="mt-0.5 px-1.5 py-0.5 rounded bg-raised border border-subtle text-[10px] text-muted font-mono">
                  {turn.backend}
                  {turn.model ? ` / ${turn.model}` : ''}
                </span>
              )}
            </div>
          ))}
          {streaming && (
            <div className="flex justify-start">
              <div className="max-w-[80%] rounded-lg px-3 py-2 text-sm whitespace-pre-wrap break-words bg-raised text-primary border border-subtle">
                {streaming.text}
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
