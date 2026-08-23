import { expect, test, type WebSocketRoute } from '@playwright/test';

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

  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/profiles') {
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
        availableBackends: [{ id: backend, displayName: backend }]
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
        ws.send(JSON.stringify({
          type: 'manager.chat.history',
          requestId: message.requestId,
          profile: message.profile,
          turns: [],
          cursor: 0
        }));
      } else if (message.type === 'manager.chat.send') {
        alphaRequestId = message.requestId;
      }
    });
  });

  await page.goto('/');
  await expect(page.locator('[role="status"]:visible', { hasText: 'Live' })).toBeVisible();
  await page.getByRole('button', { name: 'Manager Chat', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'alpha', exact: true })).toBeVisible();
  await page.getByPlaceholder(/Message the manager/).fill('alpha question');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect.poll(() => alphaRequestId).not.toBe('');

  await page.getByLabel('Node / profile').selectOption('beta');
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
  await expect(page.getByLabel('Model')).toHaveValue('beta-model');
  await page.getByPlaceholder(/Message the manager/).fill('/');
  await expect(page.getByText('/beta-command', { exact: true })).toBeVisible();
  await expect(page.getByText('org/beta · beta-backend')).toBeVisible();
});
