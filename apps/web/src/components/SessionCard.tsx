import { GitBranch, Cpu, Brain } from 'lucide-react';
import type { Session } from '@git-agent-harness/contracts';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { providerIcon } from '../lib/icons.js';

type SessionCardProps = {
  session: Session;
  onClick: () => void;
};

const STATUS_TONE: Record<Session['status'], StatusTone> = {
  idle: 'unknown',
  starting: 'warning',
  running: 'good',
  stopping: 'warning',
  stopped: 'unknown',
  error: 'critical'
};

export function SessionCard({ session, onClick }: SessionCardProps) {
  const Icon = providerIcon(session.providerKind);

  return (
    <button
      onClick={onClick}
      className="card-padded w-full text-left transition-colors hover:border-accent/40 focus-visible:outline-none"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-start gap-3 min-w-0">
          <Icon size={18} className="text-muted shrink-0 mt-0.5" aria-hidden="true" />
          <div className="min-w-0">
            <h3 className="text-sm font-semibold text-primary truncate">{session.repo || session.id}</h3>
            <p className="text-xs text-muted truncate">{session.id}</p>

            <div className="mt-2.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-secondary">
              {session.mode && <span>{session.mode}</span>}
              {session.branch && (
                <span className="inline-flex items-center gap-1">
                  <GitBranch size={12} aria-hidden="true" />
                  {session.branch}
                </span>
              )}
              {session.backend && (
                <span className="inline-flex items-center gap-1">
                  <Cpu size={12} aria-hidden="true" />
                  {session.backend}
                </span>
              )}
              {session.model && (
                <span className="inline-flex items-center gap-1">
                  <Brain size={12} aria-hidden="true" />
                  {session.model}
                </span>
              )}
            </div>
          </div>
        </div>

        <StatusBadge tone={STATUS_TONE[session.status]} label={session.status} />
      </div>

      {session.error && (
        <div className="mt-3 p-2 rounded-md text-xs text-critical" style={{ background: 'rgb(var(--status-critical) / 0.08)' }}>
          {session.error}
        </div>
      )}
    </button>
  );
}
