import { useEffect, useRef, useState } from 'react';
import { Bell, X } from 'lucide-react';
import { activityPath, type ActivityEvent } from '@git-agent-harness/contracts';
import { activityApi } from '../api/client.js';
import { formatAge } from '../lib/format.js';

/** How long a new notification stays popped open before it folds into the menu. */
const POPUP_MS = 3000;

function NotificationCard({ event, onClear, clearLabel }: { event: ActivityEvent; onClear: () => void; clearLabel: string }) {
  return (
    <div className="flex items-start gap-3 p-3">
      <Bell size={16} className="mt-0.5 shrink-0 text-accent" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-semibold text-primary">{event.title}</p>
        <p className="mt-1 line-clamp-2 text-xs text-secondary">{event.message}</p>
        <div className="mt-2 flex items-center gap-3 text-xs">
          <button type="button" className="font-medium text-accent hover:underline" onClick={() => window.location.assign(activityPath(event))}>
            {event.sessionId ? 'Open chat' : 'View activity'}
          </button>
          <span className="text-muted">{formatAge(event.occurredAt)}</span>
        </div>
      </div>
      <button type="button" className="-m-2 min-h-11 min-w-11 p-2 text-muted hover:text-primary" onClick={onClear} aria-label={clearLabel}>
        <X size={16} aria-hidden="true" />
      </button>
    </div>
  );
}

/** The bell at the right of the top navbar. A new notification pops open
 * under it for a few seconds; it then waits in the menu until it is cleared. */
export function NotificationsMenu({ liveActivity, muted = false, unreadCount, revision, autoPopup }: {
  liveActivity: ActivityEvent | null;
  /** The Activity sidebar is showing it already: count it as seen without popping it open. */
  muted?: boolean;
  unreadCount: number;
  /** Changes whenever the server's notification list does. */
  revision: number;
  autoPopup: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [popup, setPopup] = useState<ActivityEvent | null>(null);
  const [notifications, setNotifications] = useState<ActivityEvent[] | null>(null);
  const [error, setError] = useState('');
  const menu = useRef<HTMLDivElement>(null);

  // Each notification pops at most once, so closing the Activity sidebar does not bring back the last one.
  const seen = useRef<string | null>(null);
  useEffect(() => {
    if (!liveActivity || liveActivity.id === seen.current) return;
    seen.current = liveActivity.id;
    if (autoPopup && !muted) setPopup(liveActivity);
  }, [liveActivity, autoPopup, muted]);
  useEffect(() => {
    if (!popup) return;
    const timer = window.setTimeout(() => setPopup(null), POPUP_MS);
    return () => window.clearTimeout(timer);
  }, [popup]);

  useEffect(() => {
    if (!open) return;
    let current = true;
    activityApi.notifications()
      .then((result) => { if (current) { setNotifications(result.events); setError(''); } })
      .catch((err) => { if (current) setError(err instanceof Error ? err.message : String(err)); });
    return () => { current = false; };
  }, [open, revision]);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !menu.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close); };
  }, [open]);

  const clear = async (ids: string[] | 'all') => {
    const readAt = new Date().toISOString();
    setNotifications((current) => current?.map((event) => event.readAt === null && (ids === 'all' || ids.includes(event.id)) ? { ...event, readAt } : event) ?? null);
    try { await activityApi.markRead(ids); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
  };
  const unread = notifications?.filter((event) => event.readAt === null) ?? [];

  return (
    <div className="relative" ref={menu}>
      <button type="button" onClick={() => { setOpen(!open); setPopup(null); }}
        className="relative flex h-10 w-10 items-center justify-center rounded-md text-secondary hover:bg-overlay/5 hover:text-primary"
        aria-label="Notifications" aria-expanded={open} aria-haspopup="true" title="Notifications">
        <Bell size={18} aria-hidden="true" />
        {unreadCount > 0 && (
          <span className="absolute right-0 top-0 min-w-5 rounded-full bg-accent px-1.5 py-0.5 text-center text-[10px] font-semibold text-page" aria-label={`${unreadCount} unread`}>
            {Math.min(unreadCount, 99)}
          </span>
        )}
      </button>

      {popup && !open && (
        <aside className="absolute right-0 top-full z-40 mt-1 w-80 max-w-[calc(100vw-1.5rem)] rounded-lg border border-subtle bg-raised shadow-xl" aria-live="polite" aria-label="New activity">
          <NotificationCard event={popup} onClear={() => setPopup(null)} clearLabel="Dismiss notification" />
        </aside>
      )}

      {open && (
        <div className="absolute right-0 top-full z-40 mt-1 flex max-h-[min(32rem,calc(100dvh-5rem))] w-80 max-w-[calc(100vw-1.5rem)] flex-col rounded-lg border border-subtle bg-raised shadow-xl" role="region" aria-label="Notifications">
          <div className="flex items-center justify-between gap-2 border-b border-subtle px-3 py-2">
            <h2 className="text-sm font-semibold text-primary">Notifications</h2>
            {unread.length > 0 && <button type="button" className="text-xs font-medium text-accent hover:underline" onClick={() => void clear('all')}>Clear all</button>}
          </div>
          {error && <p role="alert" className="px-3 py-2 text-xs text-critical">Cannot load notifications: {error}</p>}
          {!error && unread.length === 0 && <p className="px-3 py-4 text-sm text-muted">{notifications === null ? 'Loading notifications…' : 'No new notifications.'}</p>}
          <ul className="min-h-0 divide-y divide-subtle overflow-y-auto">
            {unread.map((event) => (
              <li key={event.id}><NotificationCard event={event} onClear={() => void clear([event.id])} clearLabel={`Clear ${event.title}`} /></li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
