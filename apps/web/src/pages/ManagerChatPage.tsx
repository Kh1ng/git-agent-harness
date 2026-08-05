import { useEffect, useRef, useState } from 'react';
import { Send, MessageSquare } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { gahApi } from '../api/client.js';
import { generateRequestId } from '@git-agent-harness/shared';
import type { ManagerChatTurn } from '@git-agent-harness/contracts';

interface ChatTurn {
  role: 'user' | 'assistant' | 'system' | 'error';
  text: string;
}

function fromServerTurn(turn: ManagerChatTurn): ChatTurn {
  return { role: turn.role, text: turn.text };
}

export function ManagerChatPage() {
  const { sendMessage, messages, isConnected, reconnectSeq } = useWebSocket();
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile ?? 'gah';

  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [draft, setDraft] = useState('');
  const [pendingRequestId, setPendingRequestId] = useState<string | null>(null);
  const [historyLoaded, setHistoryLoaded] = useState(false);
  const [activeBackend, setActiveBackend] = useState<string | null>(null);
  const processedRequestIds = useRef(new Set<string>());
  const historyRequestId = useRef<string | null>(null);
  const scrollAnchorRef = useRef<HTMLDivElement>(null);

  // Restore history on mount, on profile change, and after a reconnect --
  // otherwise leaving the page (or a dropped connection) silently loses the
  // conversation even though the server keeps it.
  useEffect(() => {
    if (!isConnected) return;
    setHistoryLoaded(false);
    const requestId = generateRequestId();
    historyRequestId.current = requestId;
    sendMessage({ type: 'manager.chat.historyRequest', requestId, profile });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile, isConnected, reconnectSeq]);

  useEffect(() => {
    gahApi
      .getManagerChatSettings()
      .then((settings) => {
        const backendId = settings.profileOverrides[profile] ?? settings.defaultBackend;
        const info = settings.availableBackends.find((b) => b.id === backendId);
        setActiveBackend(info?.displayName ?? backendId);
      })
      .catch(() => setActiveBackend(null));
  }, [profile]);

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
      if (last.cleared) {
        setTurns([{ role: 'system', text: last.reply }]);
      } else {
        setTurns((prev) => [...prev, { role: 'assistant', text: last.reply }]);
      }
    } else if (last.type === 'error') {
      setTurns((prev) => [...prev, { role: 'error', text: last.error }]);
    }
    setPendingRequestId(null);
  }, [messages, pendingRequestId]);

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [turns, pendingRequestId]);

  const handleSend = () => {
    const text = draft.trim();
    if (!text || pendingRequestId) return;
    const requestId = generateRequestId();
    setTurns((prev) => [...prev, { role: 'user', text }]);
    setDraft('');
    setPendingRequestId(requestId);
    sendMessage({ type: 'manager.chat.send', requestId, profile, message: text });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Manager Chat"
        description={`Talking to ${activeBackend ?? 'the manager'} for profile "${profile}" — try /clear or /compact`}
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
              <p className="text-sm">Ask the manager about this profile's status, blockers, or next actions.</p>
            </div>
          )}
          {turns.map((turn, i) => (
            <div key={i} className={`flex ${turn.role === 'user' ? 'justify-end' : 'justify-start'}`}>
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
            </div>
          ))}
          {pendingRequestId && (
            <div className="flex justify-start">
              <div className="max-w-[80%] rounded-lg px-3 py-2 text-sm bg-raised text-muted border border-subtle animate-pulse">
                Thinking…
              </div>
            </div>
          )}
          <div ref={scrollAnchorRef} />
        </div>

        <div className="flex items-end gap-2 pt-3 mt-3 border-t border-subtle">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder={isConnected ? 'Message the manager… (/clear, /compact)' : 'Not connected to server'}
            disabled={!isConnected}
            rows={2}
            className="flex-1 bg-raised border border-subtle rounded-md px-3 py-2 text-sm text-primary resize-none disabled:opacity-50"
          />
          <button
            onClick={handleSend}
            disabled={!isConnected || !draft.trim() || !!pendingRequestId}
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
