import { useEffect, useState } from 'react';
import { Radio, CircleDot, PlayCircle, CheckCircle2, XCircle, Clock, ShieldAlert, Ban, StopCircle } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { formatLocalTime, formatAge } from '../lib/format.js';

const EVENTS_REFRESH_MS = 30 * 1000;

/** `details` is free-form text from the controller (src/events.rs) --
 * for the routine per-tick "observation_completed" event it's always
 * exactly this pattern, which repeats the profile name already shown
 * elsewhere on the page and adds nothing. Every other event's details
 * (human_required reasons, dispatch decisions, etc.) are real content
 * and still render as-is below. */
function isRedundantProfileEcho(eventType: string, details: string): boolean {
  return eventType === 'observation_completed' && /^profile=\S+$/.test(details);
}

const EVENT_ICON: Record<string, LucideIcon> = {
  observation_completed: CircleDot,
  action_decided: PlayCircle,
  dispatch_started: PlayCircle,
  dispatch_finished: CheckCircle2,
  backend_marked_unavailable: Ban,
  wait_selected: Clock,
  human_required: ShieldAlert,
  duplicate_guard_triggered: Ban,
  loop_stopped: StopCircle
};

const SINCE_OPTIONS = [
  { value: '24h', label: 'Last 24 hours' },
  { value: '7d', label: 'Last 7 days' },
  { value: '30d', label: 'Last 30 days' }
];

export function EventsPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile;
  const events = useGahStore((s) => s.events);
  const fetchEvents = useGahStore((s) => s.fetchEvents);
  const [since, setSince] = useState('24h');

  useEffect(() => {
    fetchEvents({ profile: profile ?? undefined, since }, { force: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [since, profile]);

  const refresh = () => fetchEvents({ profile: profile ?? undefined, since }, { force: true });
  useAutoRefresh(refresh, EVENTS_REFRESH_MS);
  useWsReconnectRefresh(refresh);

  const list = events.data ?? [];

  return (
    <div className="space-y-6">
      <PageHeader
        title="Events"
        description="Controller event stream: observations, decisions, dispatches, waits"
        onRefresh={refresh}
        refreshing={events.loading}
        lastUpdated={events.fetchedAt}
        actions={
          <select
            value={since}
            onChange={(e) => setSince(e.target.value)}
            className="bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary"
          >
            {SINCE_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
        }
      />

      {events.loading && !events.data ? (
        <LoadingState label="Loading events…" />
      ) : events.error ? (
        <ErrorState
          message={events.error}
          endpoint="/api/events"
          onRetry={() => fetchEvents({ profile: profile ?? undefined, since }, { force: true })}
        />
      ) : list.length === 0 ? (
        <EmptyState icon={Radio} title="No events in this window" description="The controller hasn't run recently, or hasn't logged anything yet." />
      ) : (
        <ol className="space-y-1.5">
          {list
            .slice()
            .reverse()
            .map((event, i) => {
              const Icon = EVENT_ICON[event.event_type] ?? CircleDot;
              const localTime = formatLocalTime(event.timestamp);
              const age = formatAge(event.timestamp);
              const showDetails = event.details && !isRedundantProfileEcho(event.event_type, event.details);
              return (
                <li key={i} className="card-padded flex items-start gap-3 py-2.5">
                  <Icon size={15} className="text-secondary shrink-0 mt-0.5" aria-hidden="true" />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-baseline gap-x-2">
                      <span className="text-sm font-medium text-primary">{event.event_type.replace(/_/g, ' ')}</span>
                      {event.work_id && <span className="text-xs text-accent font-mono">{event.work_id}</span>}
                      <span className="text-xs text-muted ml-auto" title={event.timestamp}>
                        {localTime ?? event.timestamp}
                        {age && <span className="text-muted/70"> · {age}</span>}
                      </span>
                    </div>
                    {showDetails && <p className="text-xs text-secondary mt-0.5 break-words">{event.details}</p>}
                  </div>
                </li>
              );
            })}
        </ol>
      )}
    </div>
  );
}
