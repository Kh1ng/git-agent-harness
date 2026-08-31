import { expect, test } from '@playwright/test';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

// #865: Hermes pushes real per-turn context-window occupancy over ACP's
// usage_update notification. The manager-chat header shows it as a small
// badge next to the model picker for a backend that reports it, and hides
// it entirely for one that doesn't -- same empty-state pattern as the
// model/reasoning-effort pickers just next to it.
test('the chat header shows and hides the context-usage badge based on backend data', async ({ page }) => {
  let contextUsage: { size: number; used: number } | null = { size: 131072, used: 12594 };

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
      return route.fulfill({ json: {
        defaultBackend: 'hermes',
        profileOverrides: {},
        availableBackends: [{ id: 'hermes', displayName: 'Hermes', implemented: true }]
      } });
    }
    if (url.pathname === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (url.pathname === '/api/manager-chat/models') {
      return route.fulfill({ json: {
        models: [],
        currentModelId: null,
        reasoningEfforts: [],
        currentReasoningEffortId: null,
        contextUsage
      } });
    }
    return route.continue();
  });

  await page.routeWebSocket('**/ws**', (ws) => {
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
      }
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'alpha', exact: true })).toBeVisible();

  const badge = page.getByLabel('Context usage');
  await expect(badge).toBeVisible();
  await expect(badge).toHaveText('10% context');
  await expect(badge).toHaveAttribute('title', '12,594 / 131,072 tokens in context');

  // A backend that reports no usage_update (e.g. Codex, Claude) hides the
  // badge outright rather than showing a stale or fake value; the page's
  // background poll picks up the change without a reload.
  contextUsage = null;
  await expect(badge).toBeHidden({ timeout: 15_000 });
});
