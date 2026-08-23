import { useEffect, useMemo, useRef, useState } from 'react';
import { Database, MessageSquare, RefreshCw, Send, Server } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { gahApi } from '../api/client.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type {
  ManagerChatSettingsSummary,
  ManagerChatTurn,
  ManagerCommandInfo,
  ManagerModelInfo,
  NodeObservationSnapshot,
  ProfileSummary
} from '@git-agent-harness/contracts';

interface ChatTurn {
  role: 'user' | 'assistant' | 'system' | 'error';
  text: string;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
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
  const [pendingRequestId, setPendingRequestId] = useState<string | null>(null);
  const [historyLoaded, setHistoryLoaded] = useState(false);
  const [chatSettings, setChatSettings] = useState<ManagerChatSettingsSummary | null>(null);
  const [controlsProfile, setControlsProfile] = useState<string | null>(null);
  const [backendChanging, setBackendChanging] = useState(false);
  const [controlError, setControlError] = useState<string | null>(null);
  const [commands, setCommands] = useState<ManagerCommandInfo[]>([]);
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [currentModelId, setCurrentModelId] = useState<string | null>(null);
  const [modelChanging, setModelChanging] = useState(false);
  const [nodes, setNodes] = useState<NodeObservationSnapshot[]>([]);
  const [nodesLoading, setNodesLoading] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const processedRequestIds = useRef(new Set<string>());
  const historyRequestId = useRef<string | null>(null);
  const backendLoadSeq = useRef(0);
  const nodeLoadSeq = useRef(0);
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
    setTurns([]);
    setHistoryLoaded(false);
    if (!isConnected) return;
    const requestId = generateRequestId();
    historyRequestId.current = requestId;
    sendMessage({ type: 'manager.chat.historyRequest', requestId, profile });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, isConnected, reconnectSeq]);

  const loadBackendControls = async (targetProfile = profile) => {
    const requestSeq = ++backendLoadSeq.current;
    const settings = await gahApi.getManagerChatSettings();
    // Real commands from the active backend's own registry (e.g. Hermes's
    // live ACP available-commands push) -- not a list GAH invents. Fetched
    // eagerly so the "/" palette has data the moment the user types it;
    // this also happens to be what warms up the backend's session.
    const commandsRequest = gahApi
      .getManagerChatCommands(targetProfile)
      .then(({ commands }) => commands)
      .catch(() => []);
    // Real selectable models from the backend's own ACP session state --
    // empty for backends that don't expose this (e.g. Claude's bridge
    // today), in which case no picker renders at all.
    const modelsRequest = gahApi
      .getManagerChatModels(targetProfile)
      .catch(() => ({ models: [], currentModelId: null }));
    const [nextCommands, nextModels] = await Promise.all([commandsRequest, modelsRequest]);
    if (requestSeq !== backendLoadSeq.current || targetProfile !== activeProfileRef.current) return;
    setChatSettings(settings);
    setCommands(nextCommands);
    setModels(nextModels.models);
    setCurrentModelId(nextModels.currentModelId);
    setControlsProfile(targetProfile);
  };

  const loadNodes = async (targetProfile = profile) => {
    const requestSeq = ++nodeLoadSeq.current;
    setNodesLoading(true);
    try {
      const nextNodes = await gahApi.getFleet(targetProfile);
      if (requestSeq === nodeLoadSeq.current) setNodes(nextNodes);
    } catch (err) {
      if (requestSeq === nodeLoadSeq.current) {
        setNodes([]);
        setControlError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (requestSeq === nodeLoadSeq.current) setNodesLoading(false);
    }
  };

  useEffect(() => {
    setControlError(null);
    setControlsProfile(null);
    setCommands([]);
    setModels([]);
    setCurrentModelId(null);
    loadBackendControls(profile).catch((err) => {
      setControlError(err instanceof Error ? err.message : String(err));
    });
    loadNodes(profile);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile]);

  const activeBackendId = chatSettings
    ? chatSettings.profileOverrides[profile] ?? chatSettings.defaultBackend
    : '';
  const activeBackend = chatSettings?.availableBackends.find((backend) => backend.id === activeBackendId);
  const controlsReady = controlsProfile === profile;

  const handleBackendChange = async (backendId: string) => {
    if (!chatSettings || backendId === activeBackendId) return;
    setBackendChanging(true);
    setControlError(null);
    const nextSettings = {
      ...chatSettings,
      profileOverrides: { ...chatSettings.profileOverrides, [profile]: backendId }
    };
    try {
      await gahApi.setManagerChatSettings({ profileOverrides: nextSettings.profileOverrides });
      if (profile !== activeProfileRef.current) return;
      setChatSettings(nextSettings);
      setCommands([]);
      setModels([]);
      setCurrentModelId(null);
      await loadBackendControls(profile);
    } catch (err) {
      setControlError(`Failed to switch backend: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBackendChanging(false);
    }
  };

  const handleModelChange = async (modelId: string) => {
    setModelChanging(true);
    try {
      await gahApi.setManagerChatModel(profile, modelId);
      if (profile !== activeProfileRef.current) return;
      setCurrentModelId(modelId);
    } catch (err) {
      setTurns((prev) => [...prev, { role: 'error', text: `Failed to switch model: ${err instanceof Error ? err.message : String(err)}` }]);
    } finally {
      setModelChanging(false);
    }
  };

  useEffect(() => {
    const last = messages[messages.length - 1];
    if (!last) return;

    if (last.type === 'manager.chat.history' && last.requestId === historyRequestId.current) {
      setTurns(last.turns.map(fromServerTurn));
      setHistoryLoaded(true);
      return;
    }

    if (!pendingRequestId) return;
    if (!('requestId' in last) || last.requestId !== pendingRequestId) return;
    if (processedRequestIds.current.has(pendingRequestId)) return;
    processedRequestIds.current.add(pendingRequestId);

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
    setPendingRequestId(null);
  }, [messages, pendingRequestId]);

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [turns, pendingRequestId]);

  // "/" palette: matches commands by prefix against whatever's typed after
  // the leading slash, only while the draft is exactly a slash-command in
  // progress (not once the user has moved on to a space-separated arg or a
  // second word).
  const paletteQuery = /^\/([a-zA-Z0-9_-]*)$/.exec(draft)?.[1];
  const paletteMatches = useMemo(() => {
    if (paletteQuery === undefined || !controlsReady) return [];
    return commands.filter((cmd) => cmd.name.startsWith(paletteQuery.toLowerCase()));
  }, [commands, controlsReady, paletteQuery]);

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
    if (!text || pendingRequestId) return;
    const requestId = generateRequestId();
    setTurns((prev) => [...prev, { role: 'user', text }]);
    setDraft('');
    setPaletteOpen(false);
    setPendingRequestId(requestId);
    sendMessage({ type: 'manager.chat.send', requestId, profile, message: text });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Manager Chat"
        description="One project conversation, portable across backends."
      />

      <div className="card overflow-hidden min-h-[620px] lg:h-[72vh] lg:grid lg:grid-cols-[224px_minmax(0,1fr)]">
        <aside className="flex min-h-0 flex-col border-b border-subtle bg-raised/30 lg:border-b-0 lg:border-r">
          <div className="border-b border-subtle px-3 py-3">
            <p className="text-xs font-semibold uppercase tracking-wide text-muted">Projects</p>
          </div>
          <nav className="flex gap-1 overflow-x-auto p-2 lg:flex-1 lg:flex-col lg:overflow-y-auto" aria-label="Project chats">
            {availableProfiles.map((item) => {
              const selected = item.name === profile;
              return (
                <button
                  key={item.name}
                  type="button"
                  onClick={() => setProfileOverride(item.name)}
                  disabled={!!pendingRequestId || backendChanging || modelChanging}
                  aria-current={selected ? 'page' : undefined}
                  className={`min-w-40 rounded-md px-3 py-2 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-50 lg:min-w-0 ${
                    selected ? 'bg-accent/15 text-primary' : 'text-secondary hover:bg-white/5 hover:text-primary'
                  }`}
                >
                  <span className="block truncate text-sm font-medium">{item.display_name || item.name}</span>
                  <span className="block truncate text-xs text-muted">{item.repo || item.name}</span>
                </button>
              );
            })}
            {availableProfiles.length === 0 && (
              <p className="px-2 py-3 text-xs text-muted">No configured projects.</p>
            )}
          </nav>

          <div className="space-y-3 border-t border-subtle p-3">
            <div>
              <div className="mb-1.5 flex items-center justify-between gap-2">
                <p className="flex items-center gap-1.5 text-xs font-medium text-secondary">
                  <Server size={13} aria-hidden="true" /> Worker nodes
                </p>
                <button
                  type="button"
                  onClick={() => loadNodes()}
                  disabled={nodesLoading}
                  className="rounded p-1 text-muted hover:bg-white/5 hover:text-primary disabled:opacity-50"
                  aria-label="Refresh worker nodes"
                >
                  <RefreshCw size={13} className={nodesLoading ? 'animate-spin' : ''} aria-hidden="true" />
                </button>
              </div>
              <div className="space-y-1">
                {nodes.map((node) => (
                  <div key={node.node_id} className="flex min-w-0 items-center justify-between gap-2 text-xs">
                    <span className="truncate text-secondary">{node.display_name}</span>
                    <span className={node.state === 'healthy' ? 'badge badge-good' : node.state === 'stale' ? 'badge badge-warning' : 'badge badge-critical'}>
                      {node.state}
                    </span>
                  </div>
                ))}
                {!nodesLoading && nodes.length === 0 && (
                  <p className="text-xs leading-5 text-muted">No registered node advertises this project.</p>
                )}
              </div>
            </div>
            <div className="flex items-start gap-2 border-t border-subtle pt-3">
              <Database size={14} className="mt-0.5 shrink-0 text-muted" aria-hidden="true" />
              <div className="min-w-0">
                <p className="text-xs font-medium text-secondary">Shared project context</p>
                <p className="truncate text-xs text-muted" title={currentProfileInfo?.repo ?? profile}>
                  {currentProfileInfo?.repo ?? profile}
                </p>
              </div>
            </div>
          </div>
        </aside>

        <section className="flex min-h-[520px] min-w-0 flex-col lg:min-h-0">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-subtle px-4 py-3">
            <div className="min-w-0">
              <h2 className="truncate text-sm font-semibold text-primary">
                {currentProfileInfo?.display_name || currentProfileInfo?.repo?.split('/').pop() || profile}
              </h2>
              <p className="truncate text-xs text-muted">{currentProfileInfo?.repo ?? profile}</p>
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <label className="sr-only" htmlFor="manager-chat-backend">Backend</label>
              <select
                id="manager-chat-backend"
                value={activeBackendId}
                onChange={(event) => handleBackendChange(event.target.value)}
                disabled={!controlsReady || !chatSettings || backendChanging || modelChanging || !!pendingRequestId}
                className="max-w-40 rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary disabled:opacity-50"
              >
                {chatSettings?.availableBackends.map((backend) => (
                  <option key={backend.id} value={backend.id} disabled={!backend.implemented}>
                    {backend.displayName}{backend.implemented ? '' : ' (unavailable)'}
                  </option>
                ))}
              </select>
              {controlsReady && models.length > 0 && (
                <>
                  <label className="sr-only" htmlFor="manager-chat-model">Model</label>
                  <select
                    id="manager-chat-model"
                    value={currentModelId ?? ''}
                    onChange={(event) => handleModelChange(event.target.value)}
                    disabled={modelChanging || backendChanging || !!pendingRequestId}
                    className="max-w-56 rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary disabled:opacity-50"
                  >
                    {models.map((model) => (
                      <option key={model.id} value={model.id}>{model.name}</option>
                    ))}
                  </select>
                </>
              )}
            </div>
          </div>

          {controlError && (
            <div className="border-b border-subtle bg-red-500/10 px-4 py-2 text-xs text-red-400" role="alert">
              {controlError} Retry from the project or node controls.
            </div>
          )}

          <div className="flex-1 space-y-3 overflow-y-auto px-4 py-4" aria-live="polite">
            {!historyLoaded && turns.length === 0 && (
              <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-muted">
                <MessageSquare size={24} className="animate-pulse opacity-50" aria-hidden="true" />
                <p className="text-sm">Loading conversation…</p>
              </div>
            )}
            {historyLoaded && turns.length === 0 && (
              <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-muted">
                <MessageSquare size={24} className="opacity-50" aria-hidden="true" />
                <p className="max-w-xl text-sm">Ask about this project's status, blockers, or next action. Type “/” for backend commands.</p>
                <p className="max-w-xl text-xs">The session log and project memory stay attached when you switch backend or model.</p>
              </div>
            )}
            {turns.map((turn, index) => (
              <div key={index} className={`flex flex-col ${turn.role === 'user' ? 'items-end' : 'items-start'}`}>
                <div
                  className={`max-w-[88%] whitespace-pre-wrap break-words rounded-lg px-3 py-2 text-sm sm:max-w-[80%] ${
                    turn.role === 'user'
                      ? 'bg-accent text-white'
                      : turn.role === 'error'
                        ? 'border border-red-500/30 bg-red-500/10 text-red-400'
                        : turn.role === 'system'
                          ? 'bg-transparent px-0 text-xs italic text-muted'
                          : 'border border-subtle bg-raised text-primary'
                  }`}
                >
                  {turn.text}
                </div>
                {turn.role === 'assistant' && turn.backend && (
                  <span className="mt-1 text-[11px] text-muted">
                    {turn.backend}{turn.model ? ` · ${turn.model}` : ''}
                  </span>
                )}
              </div>
            ))}
            {pendingRequestId && (
              <div className="flex justify-start">
                <div className="animate-pulse rounded-lg border border-subtle bg-raised px-3 py-2 text-sm text-muted">
                  {activeBackend?.displayName ?? 'Manager'} is thinking…
                </div>
              </div>
            )}
            <div ref={scrollAnchorRef} />
          </div>

          <div className="relative flex items-end gap-2 border-t border-subtle p-3 sm:p-4">
            {paletteOpen && (
              <div
                id="manager-chat-command-list"
                role="listbox"
                aria-label="Backend commands"
                className="absolute bottom-full left-3 z-10 mb-1 w-[calc(100%-1.5rem)] max-w-sm overflow-hidden rounded-md border border-subtle bg-card shadow-lg sm:left-4"
              >
                {paletteMatches.map((command, index) => (
                  <button
                    key={command.name}
                    id={`manager-chat-command-${index}`}
                    role="option"
                    aria-selected={index === paletteIndex}
                    type="button"
                    onClick={() => applyPaletteSelection(command)}
                    onMouseEnter={() => setPaletteIndex(index)}
                    className={`flex w-full items-baseline gap-2 px-3 py-2 text-left text-xs ${
                      index === paletteIndex ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'
                    }`}
                  >
                    <span className="shrink-0 font-mono">/{command.name}</span>
                    <span className="truncate opacity-80">{command.description}</span>
                  </button>
                ))}
              </div>
            )}
            <textarea
              ref={inputRef}
              aria-label="Message the manager"
              role="combobox"
              aria-haspopup="listbox"
              aria-autocomplete="list"
              aria-expanded={paletteOpen}
              aria-controls={paletteOpen ? 'manager-chat-command-list' : undefined}
              aria-activedescendant={paletteOpen ? `manager-chat-command-${paletteIndex}` : undefined}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (paletteOpen) {
                  if (event.key === 'ArrowDown') {
                    event.preventDefault();
                    setPaletteIndex((index) => (index + 1) % paletteMatches.length);
                    return;
                  }
                  if (event.key === 'ArrowUp') {
                    event.preventDefault();
                    setPaletteIndex((index) => (index - 1 + paletteMatches.length) % paletteMatches.length);
                    return;
                  }
                  if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey)) {
                    event.preventDefault();
                    applyPaletteSelection(paletteMatches[paletteIndex]);
                    return;
                  }
                  if (event.key === 'Escape') {
                    setPaletteOpen(false);
                    return;
                  }
                }
                if (event.key === 'Enter' && !event.shiftKey) {
                  event.preventDefault();
                  handleSend();
                }
              }}
              placeholder={isConnected ? `Message ${activeBackend?.displayName ?? 'the manager'}…` : 'Not connected to server'}
              disabled={!isConnected}
              rows={2}
              className="min-w-0 flex-1 resize-none rounded-md border border-subtle bg-raised px-3 py-2 text-base text-primary disabled:opacity-50 sm:text-sm"
            />
            <button
              type="button"
              onClick={handleSend}
              disabled={!isConnected || !draft.trim() || !!pendingRequestId}
              className="btn-primary h-fit shrink-0"
              aria-label="Send message"
            >
              <Send size={14} aria-hidden="true" />
            </button>
          </div>
        </section>
      </div>
    </div>
  );
}
