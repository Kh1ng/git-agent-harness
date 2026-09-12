import type { ActivityEvent } from '@git-agent-harness/contracts';

const ENABLED_KEY = 'gah.activity.systemNotifications';
const MAX_EVENTS = 200;

export function mergeActivityEvents(current: ActivityEvent[], incoming: ActivityEvent[]): ActivityEvent[] {
  const byId = new Map(current.map((event) => [event.id, event]));
  for (const event of incoming) byId.set(event.id, event);
  return [...byId.values()]
    .sort((a, b) => a.occurredAt.localeCompare(b.occurredAt))
    .slice(-MAX_EVENTS);
}

export function systemNotificationsEnabled(): boolean {
  try { return window.localStorage.getItem(ENABLED_KEY) === '1'; } catch { return false; }
}

export async function setSystemNotificationsEnabled(enabled: boolean): Promise<boolean> {
  try {
    const ios = window.webkit?.messageHandlers?.gahController;
    if (enabled && ios) {
      const granted = new Promise<boolean>((resolve) => {
        const timeout = window.setTimeout(() => finish(false), 5_000);
        const finish = (value: boolean) => {
          window.clearTimeout(timeout);
          window.removeEventListener('gah:notification-permission', onResult);
          resolve(value);
        };
        const onResult = (event: Event) => finish((event as CustomEvent<{ granted?: boolean }>).detail?.granted === true);
        window.addEventListener('gah:notification-permission', onResult, { once: true });
      });
      ios.postMessage({ type: 'requestNotifications' });
      if (!await granted) return false;
    } else if (enabled && window.__GAH_DESKTOP_SETTINGS__ !== true) {
      if (!('Notification' in window)) return false;
      if (Notification.permission !== 'granted' && await Notification.requestPermission() !== 'granted') return false;
    }
  } catch {
    return false;
  }
  try { window.localStorage.setItem(ENABLED_KEY, enabled ? '1' : '0'); } catch { /* In-app delivery still works. */ }
  return enabled;
}

export function deliverSystemNotification(event: ActivityEvent): void {
  if (!systemNotificationsEnabled()) return;
  if (window.__GAH_DESKTOP_NATIVE_NOTIFICATIONS__ === true) {
    const target = new URL('gah://notify');
    target.searchParams.set('id', event.id);
    target.searchParams.set('title', event.title);
    target.searchParams.set('body', event.message);
    const frame = document.createElement('iframe');
    frame.hidden = true;
    frame.src = target.toString();
    document.body.append(frame);
    window.setTimeout(() => frame.remove(), 1_000);
  } else if (window.webkit?.messageHandlers?.gahController) {
    window.webkit.messageHandlers.gahController.postMessage({
      type: 'activity', id: event.id, title: event.title, body: event.message
    });
  } else if ('Notification' in window && Notification.permission === 'granted' && document.hidden) {
    new Notification(event.title, { body: event.message, tag: event.id });
  }
}
