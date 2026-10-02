import { expect, test, type APIRequestContext } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

// #1241: an epic as a read-only star map, a frontier list that is the whole
// page on a phone, and planning chats seeded from the map.

async function reset(request: APIRequestContext): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name: 'normal' } });
  expect(response.ok(), await response.text()).toBe(true);
}

test('the star map shows the epic, its blockers, and the work that can start now', async ({ page, request }, testInfo) => {
  await reset(request);
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto('/?page=planning&profile=fixture');
  const map = page.getByRole('group', { name: 'Planning map for epic #900' });
  await expect(map).toBeVisible();
  await expect(page).toHaveURL(/epic=900/);

  const frontier = page.getByRole('region', { name: 'Frontier' });
  await expect(frontier.getByRole('button', { name: /#902 Local event store/ })).toBeVisible();
  await expect(frontier.getByRole('button', { name: /#905 Stale data banner/ })).toBeVisible();
  await expect(frontier.getByText('1 of 7 done under #900.')).toBeVisible();

  await map.getByRole('button', { name: /#903 Conflict resolution: Blocked/ }).click();
  const detail = page.getByRole('region', { name: 'Issue #903' });
  await expect(detail.getByText('Blocked', { exact: true })).toBeVisible();
  await expect(detail.getByText(/Blocked by/)).toContainText('#902');
  await expect(detail.getByText(/Blocks/)).toContainText('#906');
  await map.getByRole('button', { name: /#880 .*outside the epic/ }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('region', { name: 'Issue #880' }).getByText(/Outside this epic/)).toBeVisible();
  await expect(page.getByRole('region', { name: 'Issue #880' }).getByRole('button', { name: 'Discuss in chat' })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('planning-desktop.png'), fullPage: true });

  await map.getByRole('button', { name: /#903 Conflict resolution/ }).click();
  await detail.getByRole('button', { name: 'Discuss in chat' }).click();
  await expect(page).toHaveURL(/page=chat/);
  await expect(page).toHaveURL(/chat=mock-session-/);
  await expect(page).not.toHaveURL(/epic=/);
});

test('grill-me keeps where answers go and opens the planning chat', async ({ page, request }) => {
  await reset(request);
  await page.goto('/?page=planning&profile=fixture&epic=900');
  await page.getByRole('button', { name: 'Grill-me' }).click();
  const panel = page.getByRole('region', { name: 'Grill-me' });
  await expect(panel.getByRole('radio', { name: 'Issues on GitHub or GitLab' })).toBeChecked();
  const path = panel.getByRole('textbox', { name: 'Answers file path' });
  await expect(path).toBeDisabled();

  await panel.getByRole('radio', { name: /A Markdown file/ }).check();
  await path.fill('../outside.md');
  await expect(panel.getByText('Use a path inside the repository that ends in .md.')).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Start grill-me' })).toBeDisabled();
  await path.fill('plans/offline.md');
  await panel.getByRole('textbox', { name: 'What do you want to plan?' }).fill('Offline sync for the phone');
  await expect(panel.getByRole('checkbox', { name: /Plan within #900 Offline mode/ })).toBeChecked();
  await panel.getByRole('button', { name: 'Start grill-me' }).click();
  await expect(page).toHaveURL(/page=chat/);

  const saved = await request.get(`${MOCK_BASE_URL}/api/planning/settings?profile=fixture`);
  expect(await saved.json()).toEqual({ answers: 'file', path: 'plans/offline.md' });
});

test('on a phone the frontier list replaces the map', async ({ page, request }, testInfo) => {
  await reset(request);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/?page=planning&profile=fixture&epic=900');
  const frontier = page.getByRole('region', { name: 'Frontier' });
  await expect(frontier.getByRole('button', { name: /#902 Local event store/ })).toBeVisible();
  await expect(page.getByRole('group', { name: 'Planning map for epic #900' })).toBeHidden();
  await frontier.getByText(/Waiting \(3\)/).click();
  await expect(frontier.getByText('waits on #902')).toBeVisible();
  await frontier.getByRole('button', { name: /#905 Stale data banner/ }).click();
  await expect(page.getByRole('region', { name: 'Issue #905' }).getByRole('button', { name: 'Discuss in chat' })).toBeVisible();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath('planning-phone.png'), fullPage: true });
});
