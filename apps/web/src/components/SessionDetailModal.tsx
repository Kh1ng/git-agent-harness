import { useState, useRef, useEffect } from 'react';
import ReactMarkdown from 'react-markdown';
import { X, Square, Send } from 'lucide-react';
import type { Session } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { providerIcon } from '../lib/icons.js';

type SessionDetailModalProps = {
  session: Session;
  onClose: () => void;
};

const STATUS_TONE: Record<Session['status'], StatusTone> = {
  idle: 'unknown',
  starting: 'warning',
  running: 'good',
  stopping: 'warning',
  stopped: 'unknown',
  error: 'critical'
};

function Field({ label, value }: { label: string; value: string | number | undefined | null }) {
  if (value === undefined || value === null || value === '') return null;
  return (
    <div>
      <h4 className="text-xs font-medium text-muted uppercase tracking-wide mb-1">{label}</h4>
      <p className="text-sm text-primary">{value}</p>
    </div>
  );
}

export function SessionDetailModal({ session, onClose }: SessionDetailModalProps) {
  const { sessionOutput, sendMessage, isConnected } = useWebSocket();
  const [command, setCommand] = useState('');
  const outputRef = useRef<HTMLDivElement>(null);
  const output = sessionOutput[session.id];
  const Icon = providerIcon(session.providerKind);
  const isRunning = session.status === 'running';

  useEffect(() => {
    outputRef.current?.scrollTo({ top: outputRef.current.scrollHeight });
  }, [output?.stdout, output?.stderr]);

  const handleStop = () => {
    sendMessage({
      type: 'session.stop',
      requestId: `req_${Date.now()}`,
      sessionId: session.id
    });
  };

  const handleSendCommand = () => {
    const trimmed = command.trim();
    if (!trimmed) return;
    sendMessage({
      type: 'session.sendCommand',
      requestId: `req_${Date.now()}`,
      sessionId: session.id,
      command: trimmed
    });
    setCommand('');
  };

  const combinedOutput = [output?.stdout, output?.stderr].filter(Boolean).join('');

  return (
    <div className="fixed inset-0 bg-black/70 flex items-center justify-center p-4 z-50">
      <div className="card max-w-3xl w-full max-h-[90vh] overflow-hidden flex flex-col">
        <div className="flex justify-between items-center p-4 sm:p-5 border-b border-subtle">
          <div className="flex items-center gap-3 min-w-0">
            <Icon size={20} className="text-muted shrink-0" aria-hidden="true" />
            <div className="min-w-0">
              <h3 className="text-sm font-semibold text-primary truncate">{session.repo || 'Session'}</h3>
              <p className="text-xs text-muted truncate">{session.id}</p>
            </div>
          </div>

          <div className="flex items-center gap-3 shrink-0">
            <StatusBadge tone={STATUS_TONE[session.status]} label={session.status} />
            <button onClick={onClose} className="text-muted hover:text-primary" aria-label="Close">
              <X size={18} />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-4 sm:p-5">
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-4 mb-5">
            <Field label="Provider" value={session.providerKind} />
            <Field label="Mode" value={session.mode} />
            <Field label="Backend" value={session.backend} />
            <Field label="Branch" value={session.branch} />
            <Field label="Model" value={session.model} />
            <Field label="Budget" value={session.budget} />
            <Field label="Started" value={session.startedAt} />
            <Field label="Ended" value={session.endedAt} />
          </div>

          {session.error && (
            <div className="mb-5 p-3 rounded-md badge-critical" style={{ background: 'rgb(var(--status-critical) / 0.08)' }}>
              <h4 className="text-xs font-medium text-critical uppercase tracking-wide mb-1">Error</h4>
              <div className="text-sm text-primary">
                <ReactMarkdown>{session.error}</ReactMarkdown>
              </div>
            </div>
          )}

          <div>
            <h4 className="text-xs font-medium text-muted uppercase tracking-wide mb-2">Output</h4>
            <div className="terminal max-h-72 overflow-y-auto" ref={outputRef}>
              <pre className="whitespace-pre-wrap">
                {combinedOutput || 'No output received yet.'}
              </pre>
            </div>
          </div>
        </div>

        <div className="flex flex-col sm:flex-row sm:items-center gap-3 p-4 sm:p-5 border-t border-subtle bg-raised">
          {isRunning && (
            <div className="flex-1 flex items-center gap-2 min-w-0">
              <input
                type="text"
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && handleSendCommand()}
                placeholder="Send a command to this session…"
                disabled={!isConnected}
                className="flex-1 bg-page border border-subtle rounded-md px-3 py-1.5 text-sm text-primary placeholder:text-muted focus-visible:outline-none min-w-0"
              />
              <button onClick={handleSendCommand} disabled={!isConnected || !command.trim()} className="btn-secondary">
                <Send size={14} aria-hidden="true" />
                Send
              </button>
            </div>
          )}
          <div className="flex items-center gap-2 justify-end">
            {isRunning && (
              <button onClick={handleStop} disabled={!isConnected} className="btn-secondary text-critical border-critical/30">
                <Square size={14} aria-hidden="true" />
                Stop
              </button>
            )}
            <button onClick={onClose} className="btn-secondary">
              Close
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
