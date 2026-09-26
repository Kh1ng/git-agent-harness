import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

const welcome = {
  type: 'server.welcome',
  serverVersion: 'test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'gah'
};

const finished = {
  id: 'activity-1',
  occurredAt: '2026-09-12T12:00:00.000Z',
  profile: 'gah',
  kind: 'dispatch_completed',
  severity: 'success',
  title: 'Work finished',
  message: 'dispatch_ticket: success',
  workId: '#941'
};

const failed = {
  ...finished,
  id: 'activity-2',
  occurredAt: '2026-09-12T12:01:00.000Z',
  kind: 'dispatch_failed',
  severity: 'error',
  title: 'Work failed',
  message: 'dispatch_ticket: validation failed',
  workId: '#942'
};

const offline = {
  ...finished,
  id: 'activity-3',
  occurredAt: '2026-09-12T12:02:00.000Z',
  profile: null,
  kind: 'node_offline',
  severity: 'error',
  title: 'Mac worker is offline',
  message: 'The node missed three consecutive liveness checks.',
  workId: null
};

const chatFinished = {
  ...finished,
  id: 'chat:gah:session-7:3:chat_turn_completed',
  kind: 'chat_turn_completed',
  title: 'gah: reply ready',
  message: 'The requested change is ready.',
  workId: null,
  sessionId: 'session-7'
};

test('service worker always shows valid pushes and opens a window when navigation fails', async () => {
  const listeners = new Map<string, (event: never) => void>();
  const shown: unknown[][] = [];
  const opened: string[] = [];
  const client = {
    url: 'http://localhost:3000/',
    visibilityState: 'visible',
    navigate: async () => { throw new Error('navigation failed'); },
    focus: async () => undefined
  };
  const worker = {
    location: { origin: 'http://localhost:3000' },
    registration: { showNotification: async (...args: unknown[]) => { shown.push(args); } },
    clients: {
      matchAll: async () => [client],
      openWindow: async (url: string) => { opened.push(url); },
      claim: async () => undefined
    },
    skipWaiting: async () => undefined,
    addEventListener: (name: string, listener: (event: never) => void) => listeners.set(name, listener)
  };
  runInNewContext(readFileSync(new URL('../../public/sw.js', import.meta.url), 'utf8'), {
    self: worker,
    caches: {},
    fetch: async () => undefined,
    URL,
    Response
  });

  let push: Promise<void> | undefined;
  listeners.get('push')!({
    data: { json: () => ({ id: 'event-1', title: 'Ready', body: 'Done', url: '/?page=events' }) },
    waitUntil: (promise: Promise<void>) => { push = promise; }
  } as never);
  await push;
  expect(shown).toHaveLength(1);

  let click: Promise<void> | undefined;
  listeners.get('notificationclick')!({
    notification: { close: () => undefined, data: { url: 'http://localhost:3000/?page=events' } },
    waitUntil: (promise: Promise<void>) => { click = promise; }
  } as never);
  await click;
  expect(opened).toEqual(['http://localhost:3000/?page=events']);
});

test('replay de-duplicates durable activity', async ({ page }) => {
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') {
        ws.send(JSON.stringify({ type: 'activity.replay', events: [finished, finished, failed, offline] }));
      }
    });
  });
  await page.goto('/?page=events');
  await expect(page.getByRole('heading', { name: 'Activity', exact: true })).toBeVisible();
  await expect(page.getByRole('listitem').filter({ hasText: 'Work finished' })).toHaveCount(1);
  await expect(page.getByRole('listitem')).toHaveCount(3);
});

test('a live event is visible on the current surface and opens the feed', async ({ page }) => {
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') {
        ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
        ws.send(JSON.stringify({ type: 'activity.event', event: finished }));
      }
    });
  });
  await page.goto('/');
  const notice = page.getByLabel('New activity');
  await expect(notice.getByText('Work finished', { exact: true })).toBeVisible();
  await notice.getByRole('button', { name: 'View activity' }).click();
  await expect(page.getByRole('heading', { name: 'Activity', exact: true })).toBeVisible();
  await expect(page.getByRole('listitem').filter({ hasText: '#941' })).toHaveCount(1);
});

test('iOS keeps system alerts off when notification permission is denied', async ({ page }) => {
  await page.addInitScript(() => {
    window.webkit = { messageHandlers: {
      gahController: { postMessage: () => window.dispatchEvent(new CustomEvent('gah:notification-permission', { detail: { granted: false } })) }
    } };
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') {
        ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
      }
    });
  });
  await page.goto('/?page=events');
  await page.getByRole('button', { name: 'Enable system alerts' }).click();
  await expect(page.getByText('System notifications are unavailable or were not allowed.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Enable system alerts' })).toHaveAttribute('aria-pressed', 'false');
});

test('chat activity opens the originating session and suppresses a focused-chat notification', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('gah.activity.systemNotifications', '1');
    class FakeNotification {
      static permission = 'granted';
      constructor() { (window as typeof window & { notificationCount?: number }).notificationCount = 1; }
    }
    Object.defineProperty(window, 'Notification', { value: FakeNotification });
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') {
        ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
        ws.send(JSON.stringify({ type: 'activity.event', event: chatFinished }));
      }
    });
  });
  await page.goto('/?page=chat&profile=gah&chat=session-7');
  await expect.poll(() => page.evaluate(() => (window as typeof window & { notificationCount?: number }).notificationCount ?? 0)).toBe(0);
  await page.goto('/?page=events');
  await expect(page.getByRole('link', { name: /gah: reply ready/ })).toHaveAttribute('href', '/?page=chat&profile=gah&chat=session-7');
});

test('background push subscribes, unsubscribes, and hides the toggle on insecure HTTP', async ({ page }) => {
  const requests: string[] = [];
  await page.addInitScript(() => {
    sessionStorage.setItem('gah.coordinatorToken', 'push-owner-token');
    const subscription = {
      toJSON: () => ({ endpoint: 'https://push.example/device', keys: { p256dh: 'p', auth: 'a' } }),
      unsubscribe: async () => true
    };
    Object.defineProperty(window, 'PushManager', { value: class {} });
    Object.defineProperty(window, 'Notification', { value: class { static permission = 'granted'; } });
    Object.defineProperty(navigator, 'serviceWorker', { value: { ready: Promise.resolve({ pushManager: {
      getSubscription: async () => null,
      subscribe: async () => subscription
    } }) } });
  });
  await page.route('**/api/push/**', async (route) => {
    requests.push(route.request().method());
    expect(route.request().headers().authorization).toBe('Bearer push-owner-token');
    if (route.request().url().endsWith('/public-key')) return route.fulfill({ json: { publicKey: 'AQ' } });
    if (route.request().method() === 'POST') return route.fulfill({ status: 201, json: { id: 'a'.repeat(24), count: 1 } });
    if (route.request().method() === 'DELETE') return route.fulfill({ json: { removed: true, count: 0 } });
    return route.fulfill({ json: { count: 0 } });
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
    });
  });
  await page.goto('/?page=events');
  await page.getByRole('button', { name: 'Enable system alerts' }).click();
  await expect(page.getByRole('button', { name: 'System alerts on' })).toBeVisible();
  await page.getByRole('button', { name: 'System alerts on' }).click();
  await expect.poll(() => requests.filter((method) => method === 'POST').length).toBe(1);
  await expect.poll(() => requests.filter((method) => method === 'DELETE').length).toBe(1);

  await page.addInitScript(() => Object.defineProperty(window, 'isSecureContext', { value: false }));
  await page.reload();
  await expect(page.getByText('Background push needs the HTTPS dashboard URL.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Enable system alerts' })).toBeDisabled();
});

test('disabling alerts clears local state even when server cleanup fails', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('gah.activity.systemNotifications', '1');
    localStorage.setItem('gah.activity.pushSubscriptionId', 'a'.repeat(24));
    Object.defineProperty(window, 'PushManager', { value: class {} });
    Object.defineProperty(navigator, 'serviceWorker', { value: { ready: Promise.resolve({ pushManager: {
      getSubscription: async () => ({ unsubscribe: async () => {
        (window as typeof window & { pushUnsubscribed?: boolean }).pushUnsubscribed = true;
        return true;
      } })
    } }) } });
  });
  await page.route('**/api/push/subscriptions**', async (route) => {
    if (route.request().method() === 'DELETE') return route.fulfill({ status: 500, json: { message: 'store unavailable' } });
    return route.fulfill({ json: { count: 1 } });
  });
  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(welcome));
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type === 'client.hello') ws.send(JSON.stringify({ type: 'activity.replay', events: [] }));
    });
  });
  await page.goto('/?page=events');
  await page.getByRole('button', { name: 'System alerts on' }).click();
  await expect(page.getByRole('button', { name: 'Enable system alerts' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => ({
    enabled: localStorage.getItem('gah.activity.systemNotifications'),
    pushId: localStorage.getItem('gah.activity.pushSubscriptionId'),
    unsubscribed: (window as typeof window & { pushUnsubscribed?: boolean }).pushUnsubscribed
  }))).toEqual({ enabled: '0', pushId: null, unsubscribed: true });
});
