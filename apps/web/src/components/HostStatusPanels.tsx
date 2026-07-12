import React from 'react';
import { Server, Activity, AlertTriangle } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import type { HostConfig, HostStatus } from '@git-agent-harness/contracts';

interface HostStatusPanelProps {
  host: HostConfig;
  status?: HostStatus;
}

export function HostStatusPanel({ host, status }: HostStatusPanelProps) {
  const { sessions } = useWebSocket();
  const isReachable = status?.reachable ?? true;
  const snapshot = status?.snapshot;
  const blockers = snapshot?.blockers ?? [];
  const blockedWorkItems = snapshot?.blocked_work_items ?? [];
  const mergeRequests = snapshot?.merge_requests ?? [];
  const availability = snapshot?.availability ?? [];
  const errors = snapshot?.errors ?? [];
  
  // Calculate active sessions for this host
  let activeCount = status?.activeSessionCount;
  if (activeCount === undefined) {
    activeCount = sessions.filter(s => {
      const isRunning = s.status === 'running' || s.status === 'starting';
      if (host.id === 'local') {
        return isRunning && (!s.hostId || s.hostId === 'local');
      }
      return isRunning && s.hostId === host.id;
    }).length;
  }

  return (
    <div className="card-padded border border-subtle relative flex flex-col h-full bg-card rounded-lg shadow-sm">
      <div className="flex items-center justify-between border-b border-subtle pb-3 mb-3">
        <div className="flex items-center gap-2 min-w-0">
          <span className={`h-2.5 w-2.5 rounded-full shrink-0 ${isReachable ? 'bg-green-500 animate-pulse' : 'bg-red-500'}`} />
          <h4 className="font-semibold text-sm text-primary truncate" title={host.name}>{host.name}</h4>
        </div>
        <span className="text-[10px] text-muted font-mono truncate max-w-[120px]" title={host.url}>{host.url}</span>
      </div>
      
      {!isReachable ? (
        <div className="flex-1 flex flex-col items-center justify-center py-6 text-muted text-xs">
          <AlertTriangle className="text-critical mb-1" size={20} />
          <span className="font-semibold text-critical">Host Unreachable</span>
          {status?.error && <p className="text-[10px] text-critical mt-1 font-mono text-center">{status.error}</p>}
        </div>
      ) : !snapshot ? (
        <div className="flex-1 flex items-center justify-center py-6 text-muted text-xs">
          No status snapshot available
        </div>
      ) : (
        <div className="space-y-4 flex-1">
          {/* Active Sessions Count */}
          <div className="flex items-center justify-between text-xs border-b border-subtle/50 pb-2">
            <span className="text-secondary flex items-center gap-1.5">
              <Activity size={12} className="text-accent" />
              Active Sessions
            </span>
            <span className="font-semibold text-primary">{activeCount}</span>
          </div>

          {/* Blockers */}
          {(blockers.length > 0 || blockedWorkItems.length > 0 || errors.length > 0) && (
            <div>
              <p className="text-[10px] font-bold text-critical uppercase tracking-wider mb-1.5">Blockers & Errors</p>
              <ul className="space-y-1 text-xs">
                {errors.map((e, idx) => (
                  <li key={`err-${idx}`} className="text-critical flex items-start gap-1">
                    <span className="font-bold">•</span>
                    <span>[{e.subsystem}] {e.message}</span>
                  </li>
                ))}
                {blockers.map((b, idx) => (
                  <li key={`block-${idx}`} className="text-critical flex items-start gap-1">
                    <span className="font-bold">•</span>
                    <span>{b.message || b.reason}</span>
                  </li>
                ))}
                {blockedWorkItems.map((b, idx) => (
                  <li key={`work-${idx}`} className="text-warning flex items-start gap-1">
                    <span className="font-bold">•</span>
                    <span>{b.source_reference}: {b.message || b.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* Merge Requests */}
          <div>
            <p className="text-[10px] font-bold text-primary uppercase tracking-wider mb-1.5">Active MRs ({mergeRequests.length})</p>
            {mergeRequests.length === 0 ? (
              <p className="text-muted text-[11px]">No active merge requests</p>
            ) : (
              <ul className="space-y-1.5 text-xs">
                {mergeRequests.map((mr, idx) => {
                  const isReview = mr.classification === 'NEEDS_REVIEW';
                  return (
                    <li key={idx} className="flex items-center justify-between gap-2 min-w-0">
                      <span className="font-mono truncate text-secondary max-w-[150px]" title={mr.branch}>{mr.branch}</span>
                      <div className="flex items-center gap-1.5 shrink-0">
                        <span className={`px-1.5 py-0.5 rounded text-[9px] font-semibold ${
                          isReview ? 'bg-warning/10 text-warning' : 'bg-green-500/10 text-green-500'
                        }`}>
                          {mr.classification}
                        </span>
                        {mr.url && (
                          <a href={mr.url} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline text-[11px]">
                            View
                          </a>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          {/* Availability */}
          <div>
            <p className="text-[10px] font-bold text-primary uppercase tracking-wider mb-1.5">Backend Availability</p>
            {availability.length === 0 ? (
              <p className="text-muted text-[11px]">No availability constraints</p>
            ) : (
              <div className="flex flex-wrap gap-1">
                {availability.map((av, idx) => (
                  <span
                    key={idx}
                    className={`px-1.5 py-0.5 rounded text-[9px] font-mono ${
                      av.eligible_now ? 'bg-green-500/10 text-green-500' : 'bg-critical/10 text-critical'
                    }`}
                    title={av.reason || undefined}
                  >
                    {av.backend}{av.model ? `:${av.model}` : ''}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function HostStatusPanels() {
  const { hosts, hostsStatus } = useWebSocket();

  if (!hosts || hosts.length === 0) {
    return null;
  }

  return (
    <section className="space-y-3">
      <h3 className="text-sm font-semibold text-primary flex items-center gap-1.5">
        <Server size={14} className="text-accent" />
        Host Status Panels
      </h3>
      <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
        {hosts.map((host) => (
          <HostStatusPanel key={host.id} host={host} status={hostsStatus[host.id]} />
        ))}
      </div>
    </section>
  );
}
