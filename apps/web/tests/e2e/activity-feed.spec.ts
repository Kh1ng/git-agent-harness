import { expect, test } from '@playwright/test';

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
