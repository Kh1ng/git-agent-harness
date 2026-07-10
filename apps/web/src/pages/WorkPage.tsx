import { useEffect, useState } from 'react';
import { ArrowLeft, ListChecks, FileText, Rocket } from 'lucide-react';
import type { Session } from '@git-agent-harness/contracts';
import { generateProviderInstanceId } from '@git-agent-harness/shared';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { SessionCard } from '../components/SessionCard.js';
import { AttemptTimeline } from '../components/AttemptTimeline.js';

const DISPATCH_MODES = ['fix', 'improve', 'review', 'pm', 'experiment'] as const;
const DISPATCH_BACKENDS = ['auto', 'openhands', 'codex', 'claude', 'agy', 'vibe', 'opencode'] as const;

/** Minimal "start a new dispatch" form -- the dashboard could only stop/
 * command sessions that already existed, with no way to start one. Sends
 * the same `session.start` message the WS contract already defines
 * (apps/server's SessionManager.startSession); no new server-side work. */
function NewDispatchForm({ profile, repo }: { profile: string; repo: string | null }) {
  const { sendMessage, isConnected } = useWebSocket();
  const [mode, setMode] = useState<(typeof DISPATCH_MODES)[number]>('fix');
  const [backend, setBackend] = useState<(typeof DISPATCH_BACKENDS)[number]>('auto');
  const [target, setTarget] = useState('');
  const [justSent, setJustSent] = useState(false);

  const dispatch = () => {
    if (!repo) return;
    sendMessage({
      type: 'session.start',
      requestId: `dispatch_${Date.now()}`,
      profile,
      providerKind: backend,
      instanceId: generateProviderInstanceId(backend, 0),
      repo,
      mode,
      backend,
      target: target.trim() || undefined
    });
    setJustSent(true);
    setTimeout(() => setJustSent(false), 3000);
  };

  return (
    <section className="card-padded">
      <h3 className="text-sm font-semibold text-primary mb-3 flex items-center gap-2">
        <Rocket size={15} aria-hidden="true" />
        Dispatch new work
      </h3>
      <div className="flex flex-wrap items-end gap-3">
        <label className="text-xs text-muted">
          Mode
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value as typeof mode)}
            className="block mt-1 bg-raised border border-subtle rounded-md px-2 py-1.5 text-sm text-primary"
          >
            {DISPATCH_MODES.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </label>
        <label className="text-xs text-muted">
          Backend
          <select
            value={backend}
            onChange={(e) => setBackend(e.target.value as typeof backend)}
            className="block mt-1 bg-raised border border-subtle rounded-md px-2 py-1.5 text-sm text-primary"
          >
            {DISPATCH_BACKENDS.map((b) => (
              <option key={b} value={b}>{b}</option>
            ))}
          </select>
        </label>
        <label className="text-xs text-muted flex-1 min-w-[160px]">
          Target (issue number or ticket path)
          <input
            type="text"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="e.g. 148"
            className="block mt-1 w-full bg-raised border border-subtle rounded-md px-2 py-1.5 text-sm text-primary placeholder:text-muted"
          />
        </label>
        <button onClick={dispatch} disabled={!isConnected || !repo} className="btn-primary">
          {justSent ? 'Sent' : 'Dispatch'}
        </button>
      </div>
      {!repo && <p className="text-xs text-critical mt-2">No repo known for this profile yet -- check Settings.</p>}
    </section>
  );
}

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
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const [selectedWorkId, setSelectedWorkId] = useState<string | null>(null);

  useEffect(() => {
    fetchStatus(profile ?? undefined);
    fetchProfiles();
  }, [profile, fetchStatus, fetchProfiles]);

  const activeProfileRepo = profiles.data?.find((p) => p.name === profile)?.repo ?? null;

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

      <NewDispatchForm profile={profile ?? 'gah'} repo={activeProfileRepo} />

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
