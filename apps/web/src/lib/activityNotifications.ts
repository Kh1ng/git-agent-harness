import { activityPath, type ActivityEvent } from '@git-agent-harness/contracts';
import { readNavigation } from './navigationState.js';

const ENABLED_KEY = 'gah.activity.systemNotifications';
const PUSH_ID_KEY = 'gah.activity.pushSubscriptionId';
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

export function backgroundPushStatus(): 'available' | 'native' | 'https_required' | 'unsupported' {
  if (window.webkit?.messageHandlers?.gahController || window.__GAH_DESKTOP_NATIVE_NOTIFICATIONS__ === true) return 'native';
  if (!window.isSecureContext) return 'https_required';
  return 'serviceWorker' in navigator && 'PushManager' in window ? 'available' : 'unsupported';
}

function applicationServerKey(value: string): Uint8Array<ArrayBuffer> {
  const padding = '='.repeat((4 - value.length % 4) % 4);
  const bytes = atob((value + padding).replaceAll('-', '+').replaceAll('_', '/'));
  return Uint8Array.from(bytes, (character) => character.charCodeAt(0));
}

async function setBackgroundPush(enabled: boolean): Promise<void> {
  if (backgroundPushStatus() !== 'available') return;
  const registration = await navigator.serviceWorker.ready;
  const existing = await registration.pushManager.getSubscription();
  if (!enabled) {
    const id = window.localStorage.getItem(PUSH_ID_KEY);
    if (id) {
      await fetch(`/api/push/subscriptions/${encodeURIComponent(id)}`, {
        method: 'DELETE',
        headers: { 'Idempotency-Key': crypto.randomUUID() }
      });
      window.localStorage.removeItem(PUSH_ID_KEY);
    }
    await existing?.unsubscribe();
    return;
  }
  const keyResponse = await fetch('/api/push/public-key');
  if (!keyResponse.ok) throw new Error('Cannot load the push key.');
  const { publicKey } = await keyResponse.json() as { publicKey?: string };
  if (!publicKey) throw new Error('The push key is unavailable.');
  const subscription = existing ?? await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: applicationServerKey(publicKey)
  });
  const response = await fetch('/api/push/subscriptions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Idempotency-Key': crypto.randomUUID() },
    body: JSON.stringify({ subscription: subscription.toJSON(), label: navigator.userAgent.slice(0, 80) })
  });
  if (!response.ok) {
    if (!existing) await subscription.unsubscribe();
    throw new Error('Cannot save the push subscription.');
  }
  const saved = await response.json() as { id?: string };
  if (!saved.id) throw new Error('The server did not return a push subscription id.');
  window.localStorage.setItem(PUSH_ID_KEY, saved.id);
}

export async function backgroundPushDeviceCount(): Promise<number | null> {
  if (backgroundPushStatus() !== 'available') return null;
  try {
    const response = await fetch('/api/push/subscriptions');
    if (!response.ok) return null;
    const value = await response.json() as { count?: number };
    return typeof value.count === 'number' ? value.count : null;
  } catch {
    return null;
  }
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
    if (!window.webkit?.messageHandlers?.gahController && window.__GAH_DESKTOP_NATIVE_NOTIFICATIONS__ !== true) {
      await setBackgroundPush(enabled);
    }
  } catch {
    return false;
  }
  try { window.localStorage.setItem(ENABLED_KEY, enabled ? '1' : '0'); } catch { /* In-app delivery still works. */ }
  return enabled;
}

function focusedChat(event: ActivityEvent): boolean {
  if (event.kind !== 'chat_turn_completed' || document.visibilityState !== 'visible' || !document.hasFocus()) return false;
  const navigation = readNavigation();
  return navigation.page === 'chat' && navigation.profile === event.profile && navigation.chat === event.sessionId;
}

export function deliverSystemNotification(event: ActivityEvent): void {
  if (!systemNotificationsEnabled() || focusedChat(event)) return;
  const url = activityPath(event);
  if (window.__GAH_DESKTOP_NATIVE_NOTIFICATIONS__ === true) {
    const target = new URL('gah://notify');
    target.searchParams.set('id', event.id);
    target.searchParams.set('title', event.title);
    target.searchParams.set('body', event.message);
    target.searchParams.set('url', url);
    const frame = document.createElement('iframe');
    frame.hidden = true;
    frame.src = target.toString();
    document.body.append(frame);
    window.setTimeout(() => frame.remove(), 1_000);
  } else if (window.webkit?.messageHandlers?.gahController) {
    window.webkit.messageHandlers.gahController.postMessage({
      type: 'activity', id: event.id, title: event.title, body: event.message, url
    });
  } else if ('Notification' in window && Notification.permission === 'granted' && document.hidden) {
    const notification = new Notification(event.title, { body: event.message, tag: event.id });
    notification.onclick = () => {
      window.focus();
      window.location.assign(url);
      notification.close();
    };
  }
}
