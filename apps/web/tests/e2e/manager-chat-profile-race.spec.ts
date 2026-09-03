import { expect, test, type WebSocketRoute } from '@playwright/test';
import type { ServerMessage } from '@git-agent-harness/contracts';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

test('profile changes reject stale chat replies and control data', async ({ page }) => {
  let socket: WebSocketRoute;
  let alphaRequestId = '';
  let releaseAlpha!: () => void;
  const alphaGate = new Promise<void>((resolve) => { releaseAlpha = resolve; });
  let settingsCalls = 0;
  let holdAlphaHistory = true;
  let heldAlphaHistory = '';

  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/profiles') {
      return route.fulfill({ json: [
        { name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' },
        { name: 'beta', display_name: 'Beta', repo: 'org/beta' },
        { name: 'hidden', display_name: 'Hidden', repo: 'org/hidden' }
      ] });
    }
    if (url.pathname === '/api/projects') {
      return route.fulfill({ json: [
        { name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' },
        { name: 'beta', display_name: 'Beta', repo: 'org/beta' }
      ] });
    }
    if (url.pathname === '/api/controller-activity') return route.fulfill({ json: [] });
    if (url.pathname === '/api/manager-chat/settings') {
      settingsCalls += 1;
      const call = settingsCalls;
      if (call === 1) await alphaGate;
      const backend = call === 1 ? 'alpha-backend' : 'beta-backend';
      return route.fulfill({ json: {
        defaultBackend: backend,
        profileOverrides: {},
        availableBackends: [{ id: backend, displayName: backend, implemented: true }]
      } });
    }
    if (url.pathname === '/api/manager-chat/commands' || url.pathname === '/api/manager-chat/models') {
      const profile = url.searchParams.get('profile');
      if (profile === 'alpha') await alphaGate;
      if (url.pathname.endsWith('/commands')) {
        return route.fulfill({ json: { commands: [{ name: `${profile}-command`, description: profile }] } });
      }
      return route.fulfill({ json: {
        models: [{ id: `${profile}-model`, name: `${profile} model` }],
        currentModelId: `${profile}-model`
      } });
    }
    return route.continue();
  });

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.historyRequest') {
        const response = JSON.stringify({
          type: 'manager.chat.history',
          requestId: message.requestId,
          profile: message.profile,
          turns: [],
          cursor: 0,
          streaming: null
        });
        if (holdAlphaHistory && message.profile === 'alpha') heldAlphaHistory = response;
        else ws.send(response);
      } else if (message.type === 'manager.chat.send') {
        alphaRequestId = message.requestId;
      }
    });
  });

  await page.goto('/');
  await expect(page.locator('[role="status"]:visible', { hasText: 'Live' })).toBeVisible();
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'alpha', exact: true })).toBeVisible();
  const projects = page.getByRole('navigation', { name: 'Projects' });
  await expect(projects.getByRole('button', { name: 'Alpha org/alpha' })).toBeVisible();
  // The rail lists every CONFIGURED profile -- the curated catalog no longer
  // gates chat (it stays for the Overview dashboard), so an un-curated
  // profile like "hidden" is one click away (the project-switch fix).
  await expect(projects.getByRole('button', { name: 'Hidden org/hidden' })).toBeVisible();
  await expect.poll(() => heldAlphaHistory).not.toBe('');
  await expect(page.getByText('Loading conversation…')).toBeVisible();
  await expect(page.getByText('Thinking…')).toHaveCount(0);
  await page.getByPlaceholder(/Message the manager/).fill('alpha question');
  await expect(page.getByRole('button', { name: 'Send' })).toBeDisabled();
  holdAlphaHistory = false;
  socket!.send(heldAlphaHistory);
  heldAlphaHistory = '';
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
  await page.getByRole('button', { name: 'Send' }).click();
  await expect.poll(() => alphaRequestId).not.toBe('');
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: 'another-tab' }));
  await expect(page.getByText('Thinking…')).toBeVisible();
  holdAlphaHistory = true;
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: alphaRequestId }));
  await expect.poll(() => heldAlphaHistory).not.toBe('');
  await page.getByPlaceholder(/Message the manager/).fill('second question');
  // #960 replaced the in-flight Send button with Stop: while a turn is
  // running there is no Send control to disable, and the Stop affordance
  // is the proof the turn is still in flight.
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  socket!.send(heldAlphaHistory);

  await projects.getByRole('button', { name: 'Beta org/beta' }).click();
  await expect(page.getByRole('heading', { name: 'beta', exact: true })).toBeVisible();
  releaseAlpha();
  socket!.send(JSON.stringify({
    type: 'manager.chat.reply',
    requestId: alphaRequestId,
    profile: 'alpha',
    reply: 'stale alpha reply',
    backend: 'alpha-backend',
    model: 'alpha-model',
    usage: null
  }));

  await page.waitForTimeout(100);
  await expect(page.getByText('Thinking…')).toHaveCount(0);
  await expect(page.getByText('stale alpha reply')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('beta-backend · beta model');
  await page.getByPlaceholder(/Message the manager/).fill('/');
  await expect(page.getByText('/beta-command', { exact: true })).toBeVisible();
  await expect(page.getByText('org/beta', { exact: true }).first()).toBeVisible();
});

test('reconnect restores and follows an in-flight reply', async ({ page }) => {
  let historyRequests = 0;
  let complete = false;
  let socket: WebSocketRoute;

  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/profiles') return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    if (path === '/api/projects') return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    if (path === '/api/controller-activity') return route.fulfill({ json: [] });
    if (path === '/api/manager-chat/settings') return route.fulfill({ json: {
      defaultBackend: 'hermes', profileOverrides: {}, availableBackends: [{ id: 'hermes', displayName: 'Hermes', implemented: true }]
    } });
    if (path === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (path === '/api/manager-chat/models') return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.continue();
  });

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type !== 'manager.chat.historyRequest') return;
      historyRequests += 1;
      const response = JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: message.profile,
        turns: !complete
          ? [{ role: 'user', text: 'question', timestamp: 1 }]
          : [
              { role: 'user', text: 'question', timestamp: 1 },
              { role: 'assistant', text: 'complete reply', backend: 'hermes', model: null, usage: null, timestamp: 2 }
            ],
        cursor: complete ? 5 : 3,
        streaming: complete ? null : { turn: 1, partialText: 'partial reply' }
      });
      ws.send(response);
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByText('partial reply', { exact: true })).toBeVisible();
  const requestsBeforeCompletion = historyRequests;
  await page.getByPlaceholder(/Message the manager/).fill('second question');
  // #960 replaced the in-flight Send button with Stop: while a turn is
  // running there is no Send control to disable, and the Stop affordance
  // is the proof the turn is still in flight.
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  await page.waitForTimeout(500);
  expect(historyRequests).toBe(requestsBeforeCompletion);
  complete = true;
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: 'restored-turn' }));
  await expect(page.getByText('complete reply', { exact: true })).toBeVisible();
  await expect(page.getByText('partial reply', { exact: true })).toHaveCount(0);
  expect(historyRequests).toBeGreaterThan(requestsBeforeCompletion);
});

test('a cancelled turn resolves via its terminal reply and the resync shows the [cancelled] fold (#1001)', async ({ page }) => {
  let socket: WebSocketRoute;
  let sentRequestId = '';
  let folded = false;

  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/profiles') return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    if (path === '/api/projects') return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    if (path === '/api/controller-activity') return route.fulfill({ json: [] });
    if (path === '/api/manager-chat/settings') return route.fulfill({ json: {
      defaultBackend: 'hermes', profileOverrides: {}, availableBackends: [{ id: 'hermes', displayName: 'Hermes', implemented: true }]
    } });
    if (path === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (path === '/api/manager-chat/models') return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.continue();
  });

  await page.routeWebSocket('**/ws**', (ws) => {
    socket = ws;
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type === 'manager.chat.send') {
        sentRequestId = message.requestId;
        return;
      }
      if (message.type !== 'manager.chat.historyRequest') return;
      ws.send(JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: message.profile,
        turns: folded
          ? [
              { role: 'user', text: 'question', timestamp: 1 },
              { role: 'assistant', text: 'partial answer', backend: 'hermes', model: null, usage: null, timestamp: 2 },
              { role: 'system', text: '[cancelled]', timestamp: 3 }
            ]
          : [],
        cursor: folded ? 6 : 0,
        streaming: null
      }));
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();

  await page.getByPlaceholder(/Message the manager/).fill('question');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
  await page.getByRole('button', { name: 'Send' }).click();
  await expect.poll(() => sentRequestId).not.toBe('');
  await expect(page.getByText('Thinking…')).toBeVisible();

  // A tool call streams while the turn runs (slice 3 live card).
  socket!.send(JSON.stringify({
    type: 'manager.chat.toolCall',
    requestId: sentRequestId,
    profile: 'alpha',
    turn: 1,
    toolCallId: 'tc-1',
    name: 'shell',
    title: 'Shell',
    kind: 'shell',
    status: 'completed',
    locations: [],
    summary: null
  } satisfies ServerMessage));
  await expect(page.getByText('Shell', { exact: true })).toBeVisible();

  // The turn is cancelled mid-flight. #1001: the server still sends a
  // terminal reply (flagged cancelled). The client must not drop it as a new
  // assistant turn, and the busy state must resolve -- previously a cancelled
  // turn sent nothing, the pending request had already been orphaned by a
  // mid-turn resync, and the panel froze busy forever.
  socket!.send(JSON.stringify({
    type: 'manager.chat.reply',
    requestId: sentRequestId,
    profile: 'alpha',
    reply: 'partial answer',
    backend: 'hermes',
    model: null,
    usage: null,
    cancelled: true
  } satisfies ServerMessage));

  // Busy resolved: no Thinking bubble, and the input accepts a new message
  // (the send clears the draft, so retype before checking the Send enablement).
  await expect(page.getByText('Thinking…')).toHaveCount(0);
  await page.getByPlaceholder(/Message the manager/).fill('another question');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();

  // The follow-up updated forces the durable refetch, which folds the
  // cancelled turn distinctly (partial text + [cancelled] marker) and owns
  // the tool card from the live tee -- the live card must not be re-rendered
  // as a duplicate once the transcript owns it.
  folded = true;
  socket!.send(JSON.stringify({ type: 'manager.chat.updated', profile: 'alpha', requestId: sentRequestId }));
  await expect(page.getByText('[cancelled]')).toBeVisible();
  await expect(page.getByText('partial answer', { exact: true })).toBeVisible();
  await expect(page.getByText('Shell', { exact: true })).toHaveCount(1);
});
