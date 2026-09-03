import { expect, test, type WebSocketRoute } from '@playwright/test';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

// #945: the composer owns the only harness/model/effort selector. Switching
// writes a per-profile override via
// POST /api/manager-chat/settings (preserving other profiles' overrides),
// and a configured-but-unimplemented backend is flagged, not silently
// fallen back to.
test('the composer picker switches the harness and persists a per-profile override', async ({ page }) => {
  let socket: WebSocketRoute;
  let lastPost: { profileOverrides?: Record<string, string> } | null = null;
  let selectedBackend = 'hermes';
  let selectedReasoningEffort: string | null = null;

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
        selectedBackend = lastPost.profileOverrides?.alpha ?? selectedBackend;
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
    if (url.pathname === '/api/manager-chat/reasoning-effort') {
      selectedReasoningEffort = (route.request().postDataJSON() as { effortId: string }).effortId;
      return route.fulfill({ json: { success: true } });
    }
    if (url.pathname === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (url.pathname === '/api/manager-chat/models') {
      if (selectedBackend === 'claude') {
        return route.fulfill({ json: {
          models: [{ id: 'claude-model', name: 'claude model' }],
          currentModelId: 'claude-model',
          reasoningEfforts: [
            { id: 'low', name: 'Low' },
            { id: 'ultra', name: 'Ultra' }
          ],
          currentReasoningEffortId: 'low'
        } });
      }
      return route.fulfill({ json: {
        models: [],
        currentModelId: null,
        reasoningEfforts: [],
        currentReasoningEffortId: null
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

  const picker = page.getByRole('button', { name: 'Provider picker' });
  await expect(picker).toContainText('Hermes');
  await expect(page.getByLabel('Harness / backend')).toHaveCount(0);
  await picker.click();
  const controls = page.getByRole('dialog', { name: 'Provider picker' });
  // A configured-but-unimplemented backend is shown, not silently skipped.
  await expect(controls.getByRole('button', { name: 'Vibe (unavailable)', exact: true })).toBeDisabled();

  await controls.getByRole('button', { name: 'Claude', exact: true }).click();
  await expect(picker).toContainText('Claude');
  await expect(page.getByRole('paragraph').filter({ hasText: 'org/alpha' })).toBeVisible();
  await expect.poll(() => lastPost).not.toBeNull();
  expect(lastPost?.profileOverrides).toEqual({ alpha: 'claude' });

  // Capability-aware: render exactly the active backend's advertised
  // thought-level values. There is no GAH-owned low/medium/high enum.
  await expect(picker).toBeEnabled();
  await expect(controls.getByRole('button', { name: 'Low', exact: true })).toBeVisible();
  await controls.getByRole('button', { name: 'Ultra', exact: true }).click();
  await expect.poll(() => selectedReasoningEffort).toBe('ultra');

  // AC5: the picker is disabled while a turn is in flight, and re-enables
  // when the reply lands -- the busy state clears on the reply itself, not
  // only on the post-turn history reload. The reply is held until Stop is
  // observed so the in-flight state is deterministic.
  await page.getByPlaceholder(/Message the manager/).fill('question');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  await expect(picker).toBeDisabled();
  await expect(controls).toHaveCount(0);
  releaseReply();
  await expect(page.getByRole('button', { name: 'Send' })).toBeVisible();
  await expect(picker).toBeEnabled();
});
