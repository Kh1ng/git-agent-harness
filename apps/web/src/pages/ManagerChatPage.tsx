import { useEffect, useMemo, useRef, useState } from 'react';
import { Send, MessageSquare } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { gahApi } from '../api/client.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type { ManagerChatTurn, ManagerCommandInfo, ManagerModelInfo, ProfileSummary } from '@git-agent-harness/contracts';

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
  const [historyLoaded, setHistoryLoaded] = useState(false);
  const [activeBackend, setActiveBackend] = useState<string | null>(null);
  const [commands, setCommands] = useState<ManagerCommandInfo[]>([]);
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [currentModelId, setCurrentModelId] = useState<string | null>(null);
  const [modelChanging, setModelChanging] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const processedRequestIds = useRef(new Set<string>());
  const historyRequestId = useRef<string | null>(null);
  const historyPollTimer = useRef<number | null>(null);
  const activeProfileRef = useRef(profile);
  activeProfileRef.current = profile;
  const scrollAnchorRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    gahApi.getProfiles().then(setAvailableProfiles).catch(() => {});
  }, []);

  // Restore history on mount, on profile change, and after a reconnect --
  // otherwise leaving the page (or a dropped connection) silently loses the
  // conversation even though the server keeps it.
  useEffect(() => {
    if (historyPollTimer.current !== null) window.clearTimeout(historyPollTimer.current);
    setHistoryLoaded(false);
    setTurns([]);
    setPendingRequest(null);
    if (!isConnected) return;
    const requestId = generateRequestId();
    historyRequestId.current = requestId;
    sendMessage({ type: 'manager.chat.historyRequest', requestId, profile });
    return () => {
      if (historyPollTimer.current !== null) window.clearTimeout(historyPollTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, isConnected, reconnectSeq]);

  useEffect(() => {
    let cancelled = false;
    setActiveBackend(null);
    setCommands([]);
    setModels([]);
    setCurrentModelId(null);
    setModelChanging(false);
    gahApi
      .getManagerChatSettings()
      .then((settings) => {
        const backendId = settings.profileOverrides[profile] ?? settings.defaultBackend;
        const info = settings.availableBackends.find((b) => b.id === backendId);
        if (!cancelled) setActiveBackend(info?.displayName ?? backendId);
      })
      .catch(() => { if (!cancelled) setActiveBackend(null); });
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
    const last = messages[messages.length - 1];
    if (!last) return;

    if (last.type === 'manager.chat.history' && last.profile === profile && last.requestId === historyRequestId.current) {
      const restored = last.turns.map(fromServerTurn);
      if (last.streaming?.partialText) {
        restored.push({ role: 'assistant', text: last.streaming.partialText });
      }
      setTurns(restored);
      setHistoryLoaded(true);
      if (last.streaming) {
        historyPollTimer.current = window.setTimeout(() => {
          if (activeProfileRef.current !== last.profile) return;
          const requestId = generateRequestId();
          historyRequestId.current = requestId;
          sendMessage({ type: 'manager.chat.historyRequest', requestId, profile: last.profile });
        }, 250);
      }
      return;
    }

    if (!pendingRequest || pendingRequest.profile !== profile) return;
    if (!('requestId' in last) || last.requestId !== pendingRequest.id) return;
    if (processedRequestIds.current.has(pendingRequest.id)) return;
    processedRequestIds.current.add(pendingRequest.id);

    if (last.type === 'manager.chat.reply') {
      setTurns((prev) => [...prev, {
        role: 'assistant',
        text: last.reply,
        backend: last.backend,
        model: last.model
      }]);
    } else if (last.type === 'error') {
      setTurns((prev) => [...prev, { role: 'error', text: last.error }]);
    }
    setPendingRequest(null);
  }, [messages, pendingRequest, profile]);

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [turns, pendingRequest]);

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
    if (!text || pendingRequest) return;
    const requestId = generateRequestId();
    setTurns((prev) => [...prev, { role: 'user', text }]);
    setDraft('');
    setPaletteOpen(false);
    setPendingRequest({ id: requestId, profile });
    sendMessage({ type: 'manager.chat.send', requestId, profile, message: text });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title={currentProfileInfo?.repo ? currentProfileInfo.repo.split('/').pop() ?? 'Manager Chat' : 'Manager Chat'}
        description={`${currentProfileInfo?.repo ?? profile} · ${activeBackend ?? 'manager'}`}
        actions={
          <div className="flex items-center gap-2">
            {availableProfiles.length > 1 && (
              <select
                value={profileOverride ?? wsProfile ?? ''}
                onChange={(e) => setProfileOverride(e.target.value || null)}
                className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary max-w-[180px]"
                aria-label="Node / profile"
              >
                {availableProfiles.map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.display_name || p.name}
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

      <div className="card-padded flex flex-col h-[65vh]">
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
          {pendingRequest && (
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
          <button
            onClick={handleSend}
            disabled={!isConnected || !draft.trim() || !!pendingRequest}
            className="btn-primary h-fit"
            aria-label="Send"
          >
            <Send size={14} aria-hidden="true" />
          </button>
        </div>
      </div>
    </div>
  );
}
