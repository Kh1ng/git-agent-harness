import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { ServerMessage } from '@git-agent-harness/contracts';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

async function stubManagerChatApis(page: Page) {
  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/profiles' || path === '/api/projects') {
      return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    }
    if (path === '/api/controller-activity') return route.fulfill({ json: [] });
    if (path === '/api/manager-chat/sessions') return route.fulfill({ json: { sessions: [] } });
    if (path === '/api/manager-chat/settings') return route.fulfill({ json: {
      defaultBackend: 'codex',
      profileOverrides: {},
      availableBackends: [{ id: 'codex', displayName: 'Codex', implemented: true }]
    } });
    if (path === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (path === '/api/manager-chat/models') return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.continue();
  });
}

test('reconnect restores actionable activity and a durable terminal clears a missed live completion', async ({ page }) => {
  await stubManagerChatApis(page);
  let socket: WebSocketRoute;
  let terminal = false;
  let permissionResponse: Record<string, unknown> | null = null;

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.permission.respond') permissionResponse = message;
      if (message.type !== 'manager.chat.historyRequest') return;
      ws.send(JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: 'alpha',
        turns: terminal
          ? [
              { role: 'user', text: 'fix it', timestamp: 1 },
              { role: 'tool', text: 'tests passed', timestamp: 3, tool: {
                toolCallId: 'tool-1', name: 'shell', title: 'Run focused tests', kind: 'execute',
                status: 'completed', locations: ['/repo/wsServer.ts'], summary: 'tests passed'
              } },
              { role: 'assistant', text: 'finished', backend: 'codex', model: null, usage: null, timestamp: 4 }
            ]
          : [
              { role: 'user', text: 'fix it', timestamp: 1 },
              { role: 'tool', text: 'Run focused tests', timestamp: 2, tool: {
                toolCallId: 'tool-1', name: 'shell', title: 'Run focused tests', kind: 'execute',
                status: 'pending', locations: ['/repo/wsServer.ts'], summary: null
              } }
            ],
        cursor: terminal ? 8 : 4,
        streaming: terminal ? null : { turn: 1, partialText: 'working' },
        permission: terminal ? null : {
          turn: 1,
          permissionId: 'perm-1',
          title: 'Run npm test',
          options: [
            { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
            { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
          ],
          locations: ['/repo/package.json']
        }
      } satisfies ServerMessage));
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByText('working', { exact: true })).toBeVisible();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run npm test');
  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect.poll(() => permissionResponse?.permissionId).toBe('perm-1');

  socket!.send(JSON.stringify({
    type: 'manager.chat.toolCall',
    requestId: 'turn-1',
    profile: 'alpha',
    turn: 1,
    toolCallId: 'tool-1',
    name: 'shell',
    title: 'Run focused tests',
    kind: 'execute',
    status: 'completed',
    locations: ['/repo/wsServer.ts'],
    summary: 'tests passed'
  } satisfies ServerMessage));
  await expect(page.getByText('Run focused tests', { exact: true })).toHaveCount(1);

  // The terminal reply frame is intentionally missed. The durable updated →
  // history reconciliation must still remove busy/permission state.
  terminal = true;
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: 'turn-1' }));
  await expect(page.getByText('finished', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
  await page.getByPlaceholder(/Message the manager/).fill('next turn');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
});

test('permission and terminal frames remain live after the shared inbox rolls over', async ({ page }) => {
  await stubManagerChatApis(page);
  let socket: WebSocketRoute;
  let activeRequestId = '';

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.send') activeRequestId = message.requestId;
      if (message.type !== 'manager.chat.historyRequest') return;
      ws.send(JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: 'alpha',
        turns: [],
        cursor: 0,
        streaming: null,
        permission: null
      } satisfies ServerMessage));
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('long running turn');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect.poll(() => activeRequestId).not.toBe('');

  for (let seq = 1; seq <= 102; seq += 1) {
    socket!.send(JSON.stringify({
      type: 'manager.chat.chunk',
      requestId: activeRequestId,
      profile: 'alpha',
      turn: 1,
      seq,
      text: 'x'
    } satisfies ServerMessage));
  }

  socket!.send(JSON.stringify({
    type: 'manager.chat.permission',
    requestId: activeRequestId,
    profile: 'alpha',
    turn: 1,
    permissionId: 'permission-after-rollover',
    title: 'Approve after rollover',
    options: [{ optionId: 'allow-once', name: 'Allow', kind: 'allow_once' }],
    locations: ['/repo/package.json']
  } satisfies ServerMessage));
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Approve after rollover');

  socket!.send(JSON.stringify({
    type: 'manager.chat.reply',
    requestId: activeRequestId,
    profile: 'alpha',
    reply: 'finished after rollover',
    backend: 'codex',
    model: null,
    usage: null
  } satisfies ServerMessage));
  await expect(page.getByText('finished after rollover', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('the composer steers the active turn while Stop remains available, then cancellation returns it idle', async ({ page }) => {
  await stubManagerChatApis(page);
  let socket: WebSocketRoute;
  let activeRequestId = '';
  let steerMessage: Record<string, unknown> | null = null;
  let cancelMessage: Record<string, unknown> | null = null;
  let cancelled = false;

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.send') activeRequestId = message.requestId;
      if (message.type === 'manager.chat.steer') {
        steerMessage = message;
        ws.send(JSON.stringify(message.message === 'rejected direction'
          ? { type: 'error', requestId: message.requestId, error: 'backend cannot steer' }
          : {
              type: 'manager.chat.steered', requestId: message.requestId, profile: 'alpha', outcome: 'injected'
            }));
      }
      if (message.type === 'manager.chat.cancel') cancelMessage = message;
      if (message.type !== 'manager.chat.historyRequest') return;
      ws.send(JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: 'alpha',
        turns: cancelled
          ? [
              { role: 'user', text: 'start slowly', timestamp: 1 },
              { role: 'user', text: 'change direction', timestamp: 2 },
              { role: 'assistant', text: 'partial', backend: 'codex', model: null, usage: null, timestamp: 3 },
              { role: 'system', text: '[cancelled]', timestamp: 4 }
            ]
          : [],
        cursor: cancelled ? 7 : 0,
        streaming: null,
        permission: null
      } satisfies ServerMessage));
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('start slowly');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect.poll(() => activeRequestId).not.toBe('');
  socket!.send(JSON.stringify({
    type: 'manager.chat.chunk', requestId: activeRequestId, profile: 'alpha', turn: 1, seq: 3, text: 'partial'
  } satisfies ServerMessage));
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();

  await composer.fill('change direction');
  await composer.press('Enter');
  await expect.poll(() => steerMessage?.message).toBe('change direction');
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();

  await composer.fill('rejected direction');
  await composer.press('Enter');
  await expect(page.getByText('Steering failed: backend cannot steer')).toBeVisible();
  await expect(page.getByText('rejected direction', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: /^Live/ })).toBeVisible();
  await expect(page.getByText(/Connection error:/)).toHaveCount(0);

  await page.getByRole('button', { name: 'Stop' }).click();
  await expect.poll(() => cancelMessage?.profile).toBe('alpha');
  cancelled = true;
  socket!.send(JSON.stringify({
    type: 'manager.chat.reply', requestId: activeRequestId, profile: 'alpha', reply: 'partial',
    backend: 'codex', model: null, usage: null, cancelled: true
  } satisfies ServerMessage));
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: activeRequestId }));

  await expect(page.getByText('[cancelled]')).toBeVisible();
  await composer.fill('after cancel');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
});
