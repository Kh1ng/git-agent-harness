import { useEffect, useState } from 'react';
import { Bell, CheckCircle2, CircleDot, DatabaseZap, Gauge, KeyRound, MessageCircle, Radio, ShieldAlert, Wifi, WifiOff, XCircle } from 'lucide-react';
import type { ActivityKind } from '@git-agent-harness/contracts';
import type { LucideIcon } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { formatLocalTime, formatAge } from '../lib/format.js';
import { activityUrl, backgroundPushDeviceCount, backgroundPushStatus, setSystemNotificationsEnabled, systemNotificationsEnabled } from '../lib/activityNotifications.js';

const EVENT_ICON: Record<ActivityKind, LucideIcon> = {
  dispatch_completed: CheckCircle2,
  dispatch_failed: XCircle,
  review_ready: ShieldAlert,
  chat_turn_completed: MessageCircle,
  chat_turn_failed: XCircle,
  chat_permission_requested: KeyRound,
  node_offline: WifiOff,
  node_back: Wifi,
  quota_near_limit: Gauge,
  gateway_down: DatabaseZap,
  action_required: Bell
};

const SEVERITY_COLOR = {
  info: 'text-accent',
  success: 'text-good',
  warning: 'text-warning',
  error: 'text-critical'
};

export function EventsPage() {
  const { activityEvents, activitySyncedAt, isConnected, markActivityRead } = useWebSocket();
  const [systemEnabled, setSystemEnabled] = useState(systemNotificationsEnabled);
  const [permissionError, setPermissionError] = useState('');
  const [pushDevices, setPushDevices] = useState<number | null>(null);
  const pushStatus = backgroundPushStatus();

  useEffect(() => markActivityRead(), [activityEvents.length, markActivityRead]);
  useEffect(() => { void backgroundPushDeviceCount().then(setPushDevices); }, [systemEnabled]);

  const toggleSystemNotifications = async () => {
    const enabled = await setSystemNotificationsEnabled(!systemEnabled);
    setSystemEnabled(enabled);
    setPermissionError(!enabled && !systemEnabled
      ? 'System notifications are unavailable or were not allowed. The in-app feed remains active.'
      : '');
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Activity"
        description="Durable operator events, delivered live and replayed after reconnects"
        lastUpdated={activitySyncedAt}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <span className={`inline-flex items-center gap-1.5 text-xs ${isConnected ? 'text-good' : 'text-muted'}`}>
              <CircleDot size={13} aria-hidden="true" />
              {isConnected ? 'Live' : 'Reconnecting'}
            </span>
            <button
              type="button"
              className="btn-secondary"
              aria-pressed={systemEnabled}
              onClick={toggleSystemNotifications}
              disabled={pushStatus === 'https_required'}
            >
              <Bell size={14} aria-hidden="true" />
              {systemEnabled ? 'System alerts on' : 'Enable system alerts'}
            </button>
            {pushDevices !== null && <span className="text-xs text-muted">{pushDevices} subscribed device{pushDevices === 1 ? '' : 's'}</span>}
          </div>
        }
      />

      {permissionError && <p role="status" className="text-sm text-warning">{permissionError}</p>}
      {pushStatus === 'https_required' && (
        <p role="status" className="text-sm text-warning">Background push needs the HTTPS dashboard URL.</p>
      )}

      {activityEvents.length === 0 ? (
        <EmptyState
          icon={Radio}
          title={activitySyncedAt === null ? 'Waiting for activity' : 'No operator events yet'}
          description="Completed work, failures, reviews, node health, quota pressure, and gateway outages appear here."
        />
      ) : (
        <ol className="space-y-1.5" aria-label="Operator activity">
          {activityEvents.slice().reverse().map((event) => {
            const Icon = EVENT_ICON[event.kind];
            const age = formatAge(event.occurredAt);
            return (
              <li key={event.id}>
                <a href={activityUrl(event)} className="card-padded flex items-start gap-3 py-3">
                  <Icon size={16} className={`${SEVERITY_COLOR[event.severity]} shrink-0 mt-0.5`} aria-hidden="true" />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                      <span className="text-sm font-semibold text-primary">{event.title}</span>
                      {event.workId && <span className="text-xs text-accent font-mono">{event.workId}</span>}
                      <time className="text-xs text-muted sm:ml-auto" dateTime={event.occurredAt} title={event.occurredAt}>
                        {formatLocalTime(event.occurredAt) ?? event.occurredAt}
                        {age && <span className="text-muted/70"> · {age}</span>}
                      </time>
                    </div>
                    <p className="text-xs text-secondary mt-1 break-words max-w-[75ch]">{event.message}</p>
                  </div>
                </a>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
