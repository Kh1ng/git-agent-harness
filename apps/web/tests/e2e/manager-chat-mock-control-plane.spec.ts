import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

type Scenario =
  | 'normal'
  | 'slow-cancel-steer'
  | 'reconnect-stream'
  | 'reconnect-permission'
  | 'archive-success'
  | 'archive-failure'
  | 'preview-unavailable'
  | 'preview-available'
  | 'models-success'
  | 'models-empty'
  | 'models-delayed'
  | 'models-failure'
  | 'models-agy';

async function selectScenario(request: APIRequestContext, name: Scenario): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

async function connectionCount(request: APIRequestContext): Promise<number> {
  const response = await request.get(`${MOCK_BASE_URL}/api/mock/state`);
  expect(response.ok(), await response.text()).toBe(true);
  return (await response.json() as { connections: number }).connections;
}

/** Chat opens a blank conversation; these turns belong to the seeded
 * project's default conversation, which the Projects page links to. */
async function openChat(page: Page): Promise<void> {
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

/** Every existing project and chat lives on the Projects page (#1199). */
async function openProjects(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Projects', exact: true }).first().click();
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
}

async function selectSeededSession(page: Page): Promise<void> {
  await openProjects(page);
  await page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ }).click();
  // The composer pill renders the session's provider once its model list
  // has loaded (the seeded session is codex / gpt-5.3-codex).
  const pill = page.getByRole('button', { name: 'Provider picker' });
  await expect(pill).toContainText('Codex');
  await expect(pill).toContainText('GPT-5.3 Codex');
}

/** Model and provider changes go through the catalog dialog; it closes the
 * popover behind it, so callers reopen the pill when they need it again. */
async function pickFromCatalog(page: Page, popover: Locator, ...texts: string[]): Promise<void> {
  await popover.getByRole('button', { name: 'Browse all models' }).click();
  const catalog = page.getByRole('dialog', { name: 'Browse models' });
  let entry = catalog.getByRole('listitem');
  for (const text of texts) entry = entry.filter({ hasText: text });
  await entry.getByRole('button').first().click();
  await expect(catalog).not.toBeVisible();
}

async function openArchivedChats(page: Page): Promise<void> {
  const disclosure = page.getByText(/^Archived \(\d+\)$/);
  if (!await page.getByRole('navigation', { name: 'Archived chats' }).isVisible()) await disclosure.click();
}

// The mock is one shared stateful process. Keep scenario mutations ordered.
test.describe.configure({ mode: 'serial' });

test('the projects rail discovers sessions created through REST', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await openProjects(page);

  const response = await request.post(`${MOCK_BASE_URL}/api/manager-chat/sessions`, {
    data: { profile: 'fixture', backend: 'codex', title: 'Externally created session' }
  });
  expect(response.ok(), await response.text()).toBe(true);

  await expect(page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Externally created session/ })).toBeVisible({ timeout: 10_000 });
});

test('shared mock streams multiple chunks, tool activity, and completion', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('exercise normal streaming');
  await page.getByRole('button', { name: 'Send' }).click();

  await expect(page.getByText('Mock turn', { exact: false })).toBeVisible();
  await expect(page.getByText('Read fixture contracts', { exact: true })).toBeVisible();
  await expect(page.getByText('Mock turn complete after multiple chunks.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('project skills inherit, override, and restore from chat', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);

  const trigger = page.getByLabel('Project skills');
  await expect(trigger).toContainText('Skills · 1');
  await trigger.click();
  await expect(page.getByText('Inherited default · codex')).toBeVisible();
  await expect(page.getByText('Matches the latest applied turn.')).toBeVisible();
  await page.getByRole('checkbox').uncheck();
  await expect(trigger).toContainText('Skills · 0');
  await expect(page.getByText('Project override · codex')).toBeVisible();
  await expect(page.getByText('Changed since the latest applied turn. The next turn uses this selection.')).toBeVisible();
  await page.getByRole('button', { name: 'Use default' }).click();
  await expect(trigger).toContainText('Skills · 1');
  await expect(page.getByText('Inherited default · codex')).toBeVisible();
});

test('streaming resumes after the mock drops and restores the real socket', async ({ page, request }) => {
  test.setTimeout(25_000);
  await selectScenario(request, 'reconnect-stream');
  await openChat(page);
  const connectionsBeforeTurn = await connectionCount(request);

  await page.getByPlaceholder(/Message the manager/).fill('resume this turn');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Before disconnect…', { exact: true })).toBeVisible();
  await expect
    .poll(() => connectionCount(request), { timeout: 15_000 })
    .toBeGreaterThan(connectionsBeforeTurn);
  await expect(page.getByText('Before disconnect… resumed after reconnect.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('disconnect restores a pending permission card and resumes actionably', async ({ page, request }) => {
  test.setTimeout(25_000);
  await selectScenario(request, 'reconnect-permission');
  await openChat(page);
  const connectionsBeforeTurn = await connectionCount(request);

  await page.getByPlaceholder(/Message the manager/).fill('disconnect while permission is pending');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run focused Playwright tests');

  // The mock closes the real socket. Wait for the app's own reconnect loop,
  // then prove history restored both busy state and the actionable card.
  await expect
    .poll(() => connectionCount(request), { timeout: 15_000 })
    .toBeGreaterThan(connectionsBeforeTurn);
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  await expect(page.getByRole('alertdialog', { name: 'Permission request' })).toContainText('Run focused Playwright tests');

  await page.getByRole('button', { name: 'Allow', exact: true }).click();
  await expect(page.getByText('Permission allow-once received after reconnect.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('slow turn accepts/rejects steering and cancellation restores idle', async ({ page, request }) => {
  await selectScenario(request, 'slow-cancel-steer');
  await openChat(page);

  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('start slowly');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Working slowly…', { exact: true })).toBeVisible();

  await composer.fill('change direction');
  await composer.press('Enter');
  await expect(page.getByText('change direction', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();

  await composer.fill('reject this direction');
  await composer.press('Enter');
  await expect(page.getByText('Steering failed: Mock backend rejected steering')).toBeVisible();

  await page.getByRole('button', { name: 'Stop' }).click();
  await expect(page.getByText('[cancelled]', { exact: true })).toBeVisible();
  await composer.fill('after cancel');
  await expect(page.getByRole('button', { name: 'Send' })).toBeEnabled();
});

test('archive and preview states mutate through the same REST control plane', async ({ page, request }) => {
  // Walks four scenarios, a forced reconnect, and a page switch to Projects.
  test.setTimeout(90_000);
  await selectScenario(request, 'preview-unavailable');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(page.getByText('No preview yet.', { exact: false })).toBeVisible();

  await selectScenario(request, 'preview-available');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Preview :4173', exact: true }).click();
  await expect(page.getByTitle('Session preview')).toBeVisible();
  await expect(page.frameLocator('iframe[title="Session preview"]').getByRole('heading', { name: 'Mock preview available' })).toBeVisible();

  await selectScenario(request, 'archive-success');
  await openChat(page);
  await selectSeededSession(page);

  let delayNextSessionList = true;
  await page.route('**/api/manager-chat/sessions?**', async (route) => {
    if (!delayNextSessionList) return route.continue();
    delayNextSessionList = false;
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    await route.continue();
  });
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  // The selection resets optimistically, before any refresh lands.
  await expect(page.getByText('Default conversation', { exact: true })).toBeVisible({ timeout: 750 });

  const archived = await request.get(`${MOCK_BASE_URL}/api/manager-chat/sessions?profile=fixture`);
  expect(archived.ok(), await archived.text()).toBe(true);
  const archivedSessions = (await archived.json() as { sessions: { id: string; archivedAt: number | null }[] }).sessions;
  expect(archivedSessions.find((session) => session.id === 'mock-session-1')?.archivedAt).not.toBeNull();

  // The Projects rail now lists the session under Archived, not in the working set.
  await openProjects(page);
  await expect(page.getByText(/^Archived \(\d+\)$/)).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ })).toHaveCount(0);
  await openArchivedChats(page);
  await expect(page.getByRole('navigation', { name: 'Archived chats' }).getByRole('button', { name: /Mock session/ })).toBeVisible();

  // The 5s auto-refresh keeps it archived: it never resurfaces as active.
  await page.waitForTimeout(5_500);
  await expect(page.getByRole('navigation', { name: 'Archived chats' }).getByRole('button', { name: /Mock session/ })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ })).toHaveCount(0);

  await selectScenario(request, 'archive-success');
  const connectionsBeforeReconnect = await connectionCount(request);
  const rearchived = await request.post(`${MOCK_BASE_URL}/api/manager-chat/sessions/archive`, {
    data: { profile: 'fixture', sessionId: 'mock-session-1' }
  });
  expect(rearchived.ok(), await rearchived.text()).toBe(true);
  await expect.poll(() => connectionCount(request), { timeout: 30_000 }).toBeGreaterThan(connectionsBeforeReconnect);
  await expect(page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ })).toHaveCount(0);

  await selectScenario(request, 'archive-failure');
  await openChat(page);
  await selectSeededSession(page);
  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();
  await page.getByRole('button', { name: 'Archive', exact: true }).click();
  await expect(page.getByText('Failed to archive session: Mock archive failed')).toBeVisible();
  // A failed archive keeps the conversation selected, not silently reset.
  await expect(page).toHaveURL(/[?&]chat=mock-session-1/);
});

test('storage dry run selects idle sessions and bulk archives them safely', async ({ page, request }) => {
  await selectScenario(request, 'archive-success');
  await openChat(page);

  await page.getByRole('button', { name: 'Chat tools', exact: true }).click();

  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  const storage = page.getByRole('region', { name: 'Chat storage' });
  await expect(storage).toContainText('12.0 MiB in worktrees');
  await expect(storage).toContainText('12.0 MiB projected reclaim');
  await expect(storage).toContainText('archive · idle');

  await storage.getByRole('button', { name: 'Select idle (1)' }).click();
  await expect(storage.getByLabel('Select Mock session')).toBeChecked();
  await storage.getByRole('button', { name: 'Archive selected (1)' }).click();
  await expect(storage).toContainText('No live chat sessions.');

  // The archived conversation is still reachable, from Projects.
  await openProjects(page);
  await expect(page.getByText(/^Archived \(\d+\)$/)).toBeVisible({ timeout: 10_000 });
  await openArchivedChats(page);
  await expect(page.getByRole('navigation', { name: 'Archived chats' }).getByRole('button', { name: /Mock session/ })).toBeVisible();
});

test('composer provider control covers success, delayed, empty, failed, and AGY shapes', async ({ page, request }) => {
  await selectScenario(request, 'models-success');
  await openChat(page);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex · Medium');

  await selectScenario(request, 'models-delayed');
  await openChat(page);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex · Medium', { timeout: 5_000 });

  await selectScenario(request, 'models-empty');
  await openChat(page);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex');

  await selectScenario(request, 'models-failure');
  await openChat(page);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex');

  await selectScenario(request, 'models-agy');
  await openChat(page);
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('AGY');
});

test('the composer picker switches session backend, model, and effort', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await selectSeededSession(page);

  const pill = page.getByRole('button', { name: 'Provider picker' });
  await pill.click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });
  await expect(popover).toBeVisible();

  // The mock codex advertises low/medium/xhigh; effort stays in the short
  // popover, while model and provider live in the catalog (#1203). Each
  // lands as a session PATCH.
  await popover.getByRole('button', { name: 'Extra high', exact: true }).click();
  const effortState = await (await request.get(`${MOCK_BASE_URL}/api/mock/state`)).json() as {
    sessions: { id: string; reasoningEffort: string | null }[];
  };
  expect(effortState.sessions.find((s) => s.id === 'mock-session-1')?.reasoningEffort).toBe('xhigh');

  await pickFromCatalog(page, popover, 'GPT-5.3 Codex Spark');
  const modelState = await (await request.get(`${MOCK_BASE_URL}/api/mock/state`)).json() as {
    sessions: { id: string; model: string | null }[];
  };
  expect(modelState.sessions.find((s) => s.id === 'mock-session-1')?.model).toBe('gpt-5.3-codex-spark');
  await expect(pill).toContainText('GPT-5.3 Codex Spark');

  // A backend switch resets model + effort to the new backend's defaults.
  await pill.click();
  await pickFromCatalog(page, popover, 'Claude', 'Default model');
  const backendState = await (await request.get(`${MOCK_BASE_URL}/api/mock/state`)).json() as {
    sessions: { id: string; backend: string; model: string | null; reasoningEffort: string | null }[];
  };
  expect(backendState.sessions.find((s) => s.id === 'mock-session-1')).toMatchObject({
    backend: 'claude',
    model: null,
    reasoningEffort: null
  });
  await expect(pill).toContainText('Claude');

  // Escape closes the popover.
  await pill.click();
  await expect(popover).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(popover).toHaveCount(0);
});

test('composer favorites persist across reload and apply all three selections at once', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await selectSeededSession(page);

  const pill = page.getByRole('button', { name: 'Provider picker' });
  await pill.click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });

  // Build a full selection (model + effort), then star it as a favorite.
  await pickFromCatalog(page, popover, 'GPT-5.3 Codex Spark');
  await pill.click();
  await popover.getByRole('button', { name: 'Extra high', exact: true }).click();
  await expect(pill).toContainText('GPT-5.3 Codex Spark');
  await popover.getByRole('button', { name: 'Save current' }).click();
  const stored = await page.evaluate(() => window.localStorage.getItem('gah.composer.favorites'));
  expect(JSON.parse(stored ?? '[]')).toEqual([
    { backend: 'codex', model: 'gpt-5.3-codex-spark', reasoningEffort: 'xhigh' }
  ]);

  // Switch away to Claude (model/effort reset), then reload: the favorite
  // survives in localStorage and one click restores all three selections.
  await pickFromCatalog(page, popover, 'Claude', 'Default model');
  await expect(pill).toContainText('Claude · Default model');
  await page.reload();
  await openProjects(page);
  await page.getByRole('navigation', { name: 'Chats', exact: true }).getByRole('button', { name: /Mock session/ }).click();
  await expect(pill).toContainText('Claude · Default model');

  await pill.click();
  const reopened = page.getByRole('dialog', { name: 'Provider picker' });
  // The session is on Claude, so the codex favorite's model/effort names
  // can't resolve from the current lists and fall back to their raw ids.
  const favorite = reopened.getByRole('button', { name: 'Apply Codex · gpt-5.3-codex-spark · xhigh' });
  await expect(favorite).toBeVisible();
  await favorite.click();

  await expect(pill).toContainText('Codex · GPT-5.3 Codex Spark · Extra high');
  const state = await (await request.get(`${MOCK_BASE_URL}/api/mock/state`)).json() as {
    sessions: { id: string; backend: string; model: string | null; reasoningEffort: string | null }[];
  };
  expect(state.sessions.find((s) => s.id === 'mock-session-1')).toMatchObject({
    backend: 'codex',
    model: 'gpt-5.3-codex-spark',
    reasoningEffort: 'xhigh'
  });

  // A session turn still works after switching.
  const composer = page.getByPlaceholder(/Message the manager/);
  await composer.fill('after favorite switch');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.getByText('Mock turn', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop' })).toHaveCount(0);
});

test('composer favorites cannot select unavailable providers', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await page.route('**/api/manager-chat/settings', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const response = await route.fetch();
    const settings = await response.json();
    await route.fulfill({
      response,
      json: {
        ...settings,
        availableBackends: [
          ...settings.availableBackends,
          { id: 'unavailable', displayName: 'Unavailable', implemented: false },
        ],
      },
    });
  });
  await openChat(page);
  await page.evaluate(() => window.localStorage.setItem('gah.composer.favorites', JSON.stringify([{ backend: 'unavailable' }])));
  await page.reload();
  await openChat(page);
  await selectSeededSession(page);

  await page.getByRole('button', { name: 'Provider picker' }).click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });
  // A favorite pinned to an unwired provider stays listed but unusable.
  await expect(popover.getByRole('button', { name: 'Apply Unavailable' })).toBeDisabled();

  await popover.getByRole('button', { name: 'Browse all models' }).click();
  const catalog = page.getByRole('dialog', { name: 'Browse models' });
  const card = catalog.getByRole('listitem').filter({ hasText: 'Unavailable (unavailable)' });
  await expect(card.getByRole('button')).toBeDisabled();
  // And it offers no star: a provider you cannot use cannot be pinned.
  await expect(card.getByRole('button')).toHaveCount(1);
});
