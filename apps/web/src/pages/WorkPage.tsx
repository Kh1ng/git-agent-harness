import { useEffect, useState } from 'react';
import { ArrowLeft, ListChecks, FileText } from 'lucide-react';
import type { Session } from '@git-agent-harness/contracts';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { SessionCard } from '../components/SessionCard.js';
import { AttemptTimeline } from '../components/AttemptTimeline.js';

type WorkPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
};

function WorkDetail({ workId, onBack }: { workId: string; onBack: () => void }) {
  const timeline = useGahStore((s) => s.workTimelines[workId]);
  const fetchWorkTimeline = useGahStore((s) => s.fetchWorkTimeline);

  useEffect(() => {
    fetchWorkTimeline(workId);
  }, [workId, fetchWorkTimeline]);

  return (
    <div>
      <button onClick={onBack} className="inline-flex items-center gap-1.5 text-sm text-secondary hover:text-primary mb-4">
        <ArrowLeft size={15} aria-hidden="true" />
        Back to work list
      </button>
      <h2 className="text-lg font-semibold text-primary mb-1">{workId}</h2>
      <p className="text-sm text-muted mb-5">
        Dispatch → attempt → fallback → validation → repair → review → merge, in order.
      </p>

      {timeline?.loading && !timeline.data ? (
        <LoadingState label="Loading attempt history…" />
      ) : timeline?.error ? (
        <ErrorState
          message={timeline.error}
          endpoint={`/api/work/${workId}`}
          onRetry={() => fetchWorkTimeline(workId, { force: true })}
        />
      ) : !timeline?.data || timeline.data.length === 0 ? (
        <EmptyState icon={FileText} title="No ledger history for this work item yet" />
      ) : (
        <AttemptTimeline entries={timeline.data} />
      )}
    </div>
  );
}

export function WorkPage({ sessions, onSelectSession }: WorkPageProps) {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const status = useGahStore((s) => s.status);
  const fetchStatus = useGahStore((s) => s.fetchStatus);
  const [selectedWorkId, setSelectedWorkId] = useState<string | null>(null);

  useEffect(() => {
    fetchStatus(profile ?? undefined);
  }, [profile, fetchStatus]);

  if (selectedWorkId) {
    return <WorkDetail workId={selectedWorkId} onBack={() => setSelectedWorkId(null)} />;
  }

  const activeSessions = sessions.filter((s) => ['starting', 'running'].includes(s.status));
  const recentSessions = sessions.filter((s) => ['stopped', 'error'].includes(s.status)).slice(0, 5);
  const tickets = status.data?.available_tickets ?? [];

  return (
    <div className="space-y-6">
      <PageHeader
        title="Work"
        description="Active sessions and dispatchable tickets"
        onRefresh={() => fetchStatus(profile ?? undefined, { force: true })}
        refreshing={status.loading}
      />

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Active sessions ({activeSessions.length})</h3>
        {activeSessions.length === 0 ? (
          <EmptyState icon={ListChecks} title="No active sessions" />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {activeSessions.map((session) => (
              <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
            ))}
          </div>
        )}
      </section>

      {recentSessions.length > 0 && (
        <section>
          <h3 className="text-sm font-semibold text-primary mb-3">Recent sessions</h3>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {recentSessions.map((session) => (
              <SessionCard key={session.id} session={session} onClick={() => onSelectSession(session)} />
            ))}
          </div>
        </section>
      )}

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Tickets ({tickets.length})</h3>
        {status.loading && !status.data ? (
          <LoadingState label="Loading tickets…" />
        ) : status.error ? (
          <ErrorState
            message={status.error}
            endpoint="/api/status"
            onRetry={() => fetchStatus(profile ?? undefined, { force: true })}
          />
        ) : tickets.length === 0 ? (
          <EmptyState icon={ListChecks} title="No tickets found" description="docs/tickets/ is empty for this profile." />
        ) : (
          <div className="card overflow-x-auto">
            <table className="table-base min-w-[560px]">
              <thead>
                <tr>
                  <th>Ticket</th>
                  <th>Backend / model</th>
                  <th>Attempts</th>
                  <th>Status</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {tickets.map((t) => (
                  <tr key={t.ticket_path}>
                    <td className="text-primary">
                      {t.work_id && <span className="font-mono text-xs text-muted mr-1.5">{t.work_id}</span>}
                      {t.title ?? (!t.work_id ? t.ticket_path : null)}
                    </td>
                    <td>
                      {t.recommended_backend
                        ? `${t.recommended_backend}${t.recommended_model ? `/${t.recommended_model}` : ''}`
                        : 'Unknown'}
                    </td>
                    <td>{t.prior_attempt_count}</td>
                    <td>
                      {t.human_required ? (
                        <StatusBadge tone="warning" label="Human required" />
                      ) : t.has_active_mr ? (
                        <StatusBadge tone="good" label="Active MR" />
                      ) : t.prior_attempt_count > 0 ? (
                        <StatusBadge tone="serious" label={t.last_failure_class ?? 'Retrying'} />
                      ) : (
                        <StatusBadge tone="unknown" label="Not dispatched" />
                      )}
                    </td>
                    <td>
                      {t.work_id && (
                        <button
                          onClick={() => setSelectedWorkId(t.work_id!)}
                          className="text-accent hover:underline text-xs"
                        >
                          View timeline
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
