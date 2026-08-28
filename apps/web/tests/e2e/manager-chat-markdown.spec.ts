import { expect, test } from '@playwright/test';

const WELCOME = {
  type: 'server.welcome',
  serverVersion: '0.0.0-test',
  serverProviderCatalog: { providers: [] },
  sessions: [],
  providers: {},
  profile: 'alpha'
};

test('chat turns and streaming replies render markdown inside bounded bubbles', async ({ page }) => {
  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/profiles' || path === '/api/projects') {
      return route.fulfill({ json: [{ name: 'alpha', display_name: 'Alpha', repo: 'org/alpha' }] });
    }
    if (path === '/api/controller-activity') return route.fulfill({ json: [] });
    if (path === '/api/manager-chat/settings') return route.fulfill({ json: {
      defaultBackend: 'hermes',
      profileOverrides: {},
      availableBackends: [{ id: 'hermes', displayName: 'Hermes', implemented: true }]
    } });
    if (path === '/api/manager-chat/commands') return route.fulfill({ json: { commands: [] } });
    if (path === '/api/manager-chat/models') return route.fulfill({ json: { models: [], currentModelId: null } });
    return route.continue();
  });

  await page.routeWebSocket('**/ws**', (ws) => {
    ws.send(JSON.stringify(WELCOME));
    ws.onMessage((raw) => {
      const message = JSON.parse(String(raw));
      if (message.type !== 'manager.chat.historyRequest') return;
      ws.send(JSON.stringify({
        type: 'manager.chat.history',
        requestId: message.requestId,
        profile: message.profile,
        turns: [
          { role: 'user', text: 'User `inline code`', timestamp: 1 },
          {
            role: 'assistant',
            text: '**Bold assistant**\n\n- first item\n- second item\n\n[Example docs](https://example.com/docs)\n\n```ts\nconst veryLongValue = "abcdefghijklmnopqrstuvwxyz";\n```',
            backend: 'hermes',
            model: null,
            usage: null,
            timestamp: 2
          },
          { role: 'error', text: '**Bold error**', timestamp: 3 },
          { role: 'system', text: '_System note_', timestamp: 4 }
        ],
        cursor: 4,
        streaming: { turn: 5, partialText: '**Streaming reply**' }
      }));
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();

  await expect(page.locator('code', { hasText: 'inline code' })).toBeVisible();
  await expect(page.locator('strong', { hasText: 'Bold assistant' })).toBeVisible();
  await expect(page.locator('li', { hasText: 'first item' })).toBeVisible();
  await expect(page.locator('pre code.language-ts')).toContainText('veryLongValue');
  await expect(page.locator('strong', { hasText: 'Bold error' })).toBeVisible();
  await expect(page.locator('em', { hasText: 'System note' })).toBeVisible();
  await expect(page.locator('strong', { hasText: 'Streaming reply' })).toBeVisible();

  const link = page.getByRole('link', { name: 'Example docs' });
  await expect(link).toHaveAttribute('href', 'https://example.com/docs');
  await expect(link).toHaveAttribute('target', '_blank');
  await expect(link).toHaveAttribute('rel', 'noreferrer');

  const assistantBubble = page.locator('strong', { hasText: 'Bold assistant' })
    .locator('xpath=ancestor::div[contains(@class, "rounded-lg")][1]');
  await expect(assistantBubble).toHaveClass(/max-w-\[80%\]/);
  await expect(assistantBubble).not.toHaveClass(/max-w-none/);
  await expect(page.locator('pre')).toHaveCSS('overflow-x', 'auto');
});
