import { expect, test, type WebSocketRoute } from '@playwright/test';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

// #945: the harness/backend selector lives in the chat header, next to the
// model picker. Switching writes a per-profile override via
// POST /api/manager-chat/settings (preserving other profiles' overrides),
// and a configured-but-unimplemented backend is flagged, not silently
// fallen back to.
test('the chat header switches the harness and persists a per-profile override', async ({ page }) => {
  let socket: WebSocketRoute;
  let lastPost: { profileOverrides?: Record<string, string> } | null = null;

  // The chat reply is gated until the AC5 block releases it, so the
  // in-flight (Stop) state is observed deterministically.
  let releaseReply!: () => void;
  const replyGate = new Promise<void>((resolve) => { releaseReply = resolve; });

  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/profiles') {
      return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    }
    if (url.pathname === '/api/projects') {
      return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    }
    if (url.pathname === '/api/controller-activity') return route.fulfill({ json: [] });
    if (url.pathname === '/api/manager-chat/settings') {
      if (route.request().method() === 'POST') {
        lastPost = route.request().postDataJSON() as { profileOverrides?: Record<string, string> };
        return route.fulfill({ json: { success: true } });
      }
      return route.fulfill({ json: {
        defaultBackend: 'hermes',
        profileOverrides: {},
        availableBackends: [
          { id: 'hermes', displayName: 'Hermes', implemented: true },
          { id: 'claude', displayName: 'Claude', implemented: true },
          { id: 'vibe', displayName: 'Vibe', implemented: false }
        ]
      } });
    }
    if (url.pathname === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (url.pathname === '/api/manager-chat/models') {
      const backend = url.searchParams.get('profile') === 'alpha' ? 'claude' : 'hermes';
      return route.fulfill({ json: { models: [{ id: `${backend}-model`, name: `${backend} model` }], currentModelId: `${backend}-model` } });
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
          cursor: 0,
          streaming: null
        }));
      } else if (message.type === 'manager.chat.send') {
        // The reply is gated: the AC5 block releases it after asserting the
        // in-flight (Stop) state, so the busy-state transition is observed
        // deterministically instead of racing the immediate reply.
        void replyGate.then(() => {
          ws.send(JSON.stringify({
            type: 'manager.chat.reply',
            requestId: message.requestId,
            profile: message.profile,
            reply: 'ok',
            backend: 'claude',
            model: 'claude-model',
            usage: null
          }));
        });
      }
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'alpha', exact: true })).toBeVisible();

  const harness = page.getByLabel('Harness / backend');
  await expect(harness).toHaveValue('hermes');
  // A configured-but-unimplemented backend is shown, not silently skipped.
  await expect(harness.locator('option[value="vibe"]')).toHaveText('Vibe (unavailable)');

  await harness.selectOption('claude');
  await expect(harness).toHaveValue('claude');
  await expect(page.getByText('org/alpha · Claude')).toBeVisible();
  await expect.poll(() => lastPost).not.toBeNull();
  expect(lastPost?.profileOverrides).toEqual({ alpha: 'claude' });

  // AC5: the picker is disabled while a turn is in flight, and re-enables
  // when the reply lands -- the busy state clears on the reply itself, not
  // only on the post-turn history reload. The reply is held until Stop is
  // observed so the in-flight state is deterministic.
  await page.getByPlaceholder(/Message the manager/).fill('question');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  await expect(harness).toBeDisabled();
  releaseReply();
  await expect(page.getByRole('button', { name: 'Send' })).toBeVisible();
  await expect(harness).toBeEnabled();
});
