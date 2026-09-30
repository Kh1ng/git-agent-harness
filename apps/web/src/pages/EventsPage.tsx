import { useEffect, useRef, useState } from 'react';
import { Bell, CheckCircle2, CircleDot, DatabaseZap, Gauge, KeyRound, MessageCircle, Radio, ShieldAlert, Wifi, WifiOff, XCircle } from 'lucide-react';
import { activityPath, notifiableActivity, type ActivityEvent, type ActivityKind, type DeliveryReceipt } from '@git-agent-harness/contracts';
import type { LucideIcon } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { activityApi } from '../api/client.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { formatLocalTime, formatAge } from '../lib/format.js';
import { backgroundPushDeviceCount, backgroundPushStatus, setSystemNotificationsEnabled, systemNotificationsEnabled } from '../lib/activityNotifications.js';

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

type View = 'notifications' | 'all';

/** Push subscriptions are labelled with the browser's user agent; a chip
 * needs the device, not the string. */
export function deliveryTargetName(receipt: Pick<DeliveryReceipt, 'method' | 'target'>): string {
  if (receipt.method !== 'web_push') return receipt.target;
  const device = [['iPhone', /iPhone/], ['iPad', /iPad/], ['Android', /Android/], ['Mac', /Macintosh|Mac OS X/], ['Windows', /Windows/], ['Linux', /Linux/]]
    .find(([, pattern]) => (pattern as RegExp).test(receipt.target));
  return device ? `${device[0]} browser` : receipt.target.slice(0, 24);
}

/** The latest outcome per target; a retry replaces the earlier attempt. */
function latestReceipts(deliveries: DeliveryReceipt[] = []): DeliveryReceipt[] {
  const byTarget = new Map<string, DeliveryReceipt>();
  for (const receipt of deliveries) byTarget.set(`${receipt.method}:${receipt.target}`, receipt);
  return [...byTarget.values()];
}

function DeliveryChips({ deliveries }: { deliveries?: DeliveryReceipt[] }) {
  const receipts = latestReceipts(deliveries);
  if (receipts.length === 0) return null;
  return (
    <ul className="mt-1.5 flex flex-wrap gap-1.5" aria-label="Delivery">
      {receipts.map((receipt) => {
        const name = deliveryTargetName(receipt);
        return (
          <li key={`${receipt.method}:${receipt.target}`}
            className={`rounded-full border px-2 py-0.5 text-[11px] ${receipt.ok ? 'border-good/40 text-good' : 'border-critical/40 text-critical'}`}>
            {receipt.ok ? `${name} ✓` : `${name} ✗${receipt.reason ? ` (${receipt.reason})` : ''}`}
          </li>
        );
      })}
    </ul>
  );
}

function ActivityRow({ event, highlighted, onRead }: { event: ActivityEvent; highlighted: boolean; onRead: (id: string) => void }) {
  const Icon = EVENT_ICON[event.kind];
  const age = formatAge(event.occurredAt);
  const unread = notifiableActivity(event) && !event.readAt;
  const chat = !!(event.profile && event.sessionId);
  return (
    <li id={`activity-${event.id}`} data-highlighted={highlighted || undefined}>
      <div className={`card-padded flex items-start gap-3 py-3 ${highlighted ? 'ring-2 ring-accent' : ''}`}>
        <Icon size={16} className={`${SEVERITY_COLOR[event.severity]} shrink-0 mt-0.5`} aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
            {unread && <span className="h-2 w-2 shrink-0 self-center rounded-full bg-accent" role="img" aria-label="Unread" />}
            {unread
              ? <button type="button" className="text-left text-sm font-semibold text-primary" onClick={() => onRead(event.id)}>{event.title}</button>
              : <span className="text-sm font-semibold text-primary">{event.title}</span>}
            {event.workId && <span className="text-xs text-accent font-mono">{event.workId}</span>}
            <time className="text-xs text-muted sm:ml-auto" dateTime={event.occurredAt} title={event.occurredAt}>
              {formatLocalTime(event.occurredAt) ?? event.occurredAt}
              {age && <span className="text-muted/70"> · {age}</span>}
            </time>
          </div>
          <p className="text-xs text-secondary mt-1 break-words max-w-[75ch]">{event.message}</p>
          <DeliveryChips deliveries={event.deliveries} />
          {chat && <a href={activityPath(event)} className="mt-1.5 inline-flex min-h-11 items-center text-xs text-accent underline sm:min-h-0">Open chat</a>}
        </div>
      </div>
    </li>
  );
}

export function EventsPage({ openedEventId = null }: { openedEventId?: string | null }) {
  const { activityEvents, activitySyncedAt, isConnected, activityUnreadCount, activityRevision, reconnectSeq } = useWebSocket();
  const [systemEnabled, setSystemEnabled] = useState(systemNotificationsEnabled);
  const [permissionError, setPermissionError] = useState('');
  const [pushDevices, setPushDevices] = useState<number | null>(null);
  // Opened from a push, a feed link, or the unread badge: show the pings first.
  const [view, setView] = useState<View>(() => openedEventId || activityUnreadCount > 0 ? 'notifications' : 'all');
  const [notifications, setNotifications] = useState<ActivityEvent[] | null>(null);
  const [notificationsError, setNotificationsError] = useState('');
  const scrolled = useRef(false);
  const pushStatus = backgroundPushStatus();

  useEffect(() => { void backgroundPushDeviceCount().then(setPushDevices); }, [systemEnabled]);
  useEffect(() => {
    let cancelled = false;
    activityApi.notifications()
      .then(({ events }) => { if (!cancelled) { setNotifications(events); setNotificationsError(''); } })
      .catch((error) => { if (!cancelled) setNotificationsError(error instanceof Error ? error.message : String(error)); });
    return () => { cancelled = true; };
  }, [activityRevision, reconnectSeq]);

  // A routine event is only in the full list; follow the link there.
  useEffect(() => {
    if (openedEventId && notifications && !notifications.some((event) => event.id === openedEventId)
      && activityEvents.some((event) => event.id === openedEventId)) setView('all');
  }, [openedEventId, notifications, activityEvents]);

  const events = view === 'notifications' ? notifications ?? [] : activityEvents.slice().reverse();
  useEffect(() => {
    if (!openedEventId || scrolled.current || !events.some((event) => event.id === openedEventId)) return;
    scrolled.current = true;
    document.getElementById(`activity-${openedEventId}`)?.scrollIntoView({ block: 'center' });
  }, [openedEventId, events]);

  const markRead = async (ids: string[] | 'all') => {
    const readAt = new Date().toISOString();
    setNotifications((current) => current?.map((event) => ids === 'all' || ids.includes(event.id) ? { ...event, readAt: event.readAt ?? readAt } : event) ?? null);
    try { await activityApi.markRead(ids); }
    catch (error) { setNotificationsError(error instanceof Error ? error.message : String(error)); }
  };

  const toggleSystemNotifications = async () => {
    const enabled = await setSystemNotificationsEnabled(!systemEnabled);
    setSystemEnabled(enabled);
    setPermissionError(!enabled && !systemEnabled
      ? 'System notifications are unavailable or were not allowed. The in-app feed remains active.'
      : '');
  };

  const tab = (id: View, label: string) => (
    <button type="button" role="tab" aria-selected={view === id} onClick={() => setView(id)}
      className={`rounded-md px-3 py-1.5 text-xs ${view === id ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}>
      {label}
    </button>
  );

  return (
    <div className="space-y-6">
      <PageHeader
        title="Activity"
        description="Every notification you were sent, and the durable operator events behind them"
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

      <div className="flex flex-wrap items-center gap-2">
        <div role="tablist" aria-label="Activity view" className="flex gap-2">
          {tab('notifications', activityUnreadCount > 0 ? `Notifications (${activityUnreadCount})` : 'Notifications')}
          {tab('all', 'All activity')}
        </div>
        {view === 'notifications' && notifications?.some((event) => !event.readAt) && (
          <button type="button" className="btn-secondary ml-auto text-xs" onClick={() => void markRead('all')}>Mark all read</button>
        )}
      </div>

      {view === 'notifications' && notificationsError && (
        <p role="alert" className="text-sm text-critical">Cannot load notifications: {notificationsError}</p>
      )}

      {events.length === 0 ? (
        view === 'notifications'
          ? <EmptyState icon={Bell} title={notifications === null && !notificationsError ? 'Loading notifications' : 'No notifications in the last 30 days'}
              description="Replies, failures, reviews, permission requests, and offline nodes that were pushed to you appear here." />
          : <EmptyState
              icon={Radio}
              title={activitySyncedAt === null ? 'Waiting for activity' : 'No operator events yet'}
              description="Completed work, failures, reviews, node health, quota pressure, and gateway outages appear here."
            />
      ) : (
        <ol className="space-y-1.5" aria-label={view === 'notifications' ? 'Notifications' : 'Operator activity'}>
          {events.map((event) => (
            <ActivityRow key={event.id} event={event} highlighted={event.id === openedEventId} onRead={(id) => void markRead([id])} />
          ))}
        </ol>
      )}
    </div>
  );
}
