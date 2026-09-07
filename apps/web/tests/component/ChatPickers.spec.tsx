import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { ChatSessionSummary } from '@git-agent-harness/contracts';
import { ProjectRail } from '../../src/components/ProjectRail.js';
import { NewChatModal } from '../../src/components/NewChatModal.js';

const sessions: ChatSessionSummary[] = Array.from({ length: 30 }, (_, index) => ({
  id: String(index + 1), profile: 'gah', title: `Conversation ${index + 1}`,
  branch: `fix/work-${index + 1}`, prNumber: index + 1, worktreePath: null,
  backend: 'claude', model: null, reasoningEffort: null, createdAt: 0, lastActiveAt: 0,
  archivedAt: index < 15 ? null : 1, outcome: index < 15 ? 'live' : 'archived',
  settledAt: null, settledReason: null
}));

test('chat and archive navigation bounds rows, shares search, and retains the selection', async ({ mount, page }, testInfo) => {
  let selected: string | null = '15';
  const component = await mount(<ProjectRail currentProfile="gah" profiles={[]} sessions={sessions}
    selectedSessionId="15" onSelect={() => {}} onSessionSelect={(id) => { selected = id; }}
    sessionsError={false} onRetrySessions={() => {}} onProjectAdded={() => {}} />);
  const live = component.getByRole('navigation', { name: 'Chats', exact: true });
  const archived = component.getByRole('navigation', { name: 'Archived chats' });
  const filter = component.getByRole('searchbox', { name: 'Filter chats and archive' });
  await expect(live.locator('button:has(span)')).toHaveCount(10);
  await expect(live.getByRole('button', { name: 'Conversation 15', exact: false })).toHaveAttribute('aria-current', 'page');
  await expect(live.getByRole('button', { name: /Conversation 15/ })).toBeInViewport();
  await expect(archived).not.toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('gah-1116-rail-desktop.png') });
  await live.getByRole('button', { name: 'Show all' }).click();
  await expect(live.getByRole('button')).toHaveCount(17); // 16 chats and disclosure
  await live.getByRole('button', { name: 'Show fewer' }).press('Enter');
  await expect(live.getByRole('button', { name: 'Show all' })).toBeFocused();
  await filter.fill('  WORK-14  ');
  await expect(filter).toBeFocused();
  await expect(live.getByRole('button')).toHaveCount(2); // match plus current selection
  await expect(live.getByRole('status')).toContainText('Matches: 1 of 16');
  await live.getByRole('button', { name: /Conversation 14/ }).click();
  await expect.poll(() => selected).toBe('14');
  await filter.fill('missing');
  await expect(live.getByText('No chats match your search.')).toBeVisible();
  await expect(live.getByRole('button', { name: /Conversation 15/ })).toBeVisible();
  await filter.clear();
  await component.locator('summary').filter({ hasText: 'Archived (15)' }).click();
  await expect(archived.getByRole('button')).toHaveCount(11);
  await archived.getByRole('button', { name: 'Show all' }).click();
  await expect(archived.getByRole('button')).toHaveCount(16);
  await filter.fill('30');
  await expect(archived.getByRole('button')).toHaveCount(1);
  await expect(archived.getByRole('button', { name: /Conversation 30/ })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(filter).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('gah-1116-rail-mobile.png') });
  expect(await component.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test('a selected archived chat opens its disclosure and survives the ten-row bound', async ({ mount }) => {
  const component = await mount(<ProjectRail currentProfile="gah" profiles={[]} sessions={sessions}
    selectedSessionId="30" onSelect={() => {}} onSessionSelect={() => {}}
    sessionsError={false} onRetrySessions={() => {}} onProjectAdded={() => {}} />);
  const archived = component.getByRole('navigation', { name: 'Archived chats' });
  await expect(archived.getByRole('button')).toHaveCount(11);
  await expect(archived.getByRole('button', { name: /Conversation 30/ })).toBeVisible();
  await expect(archived.getByRole('button', { name: /Conversation 30/ })).toHaveAttribute('aria-current', 'page');
});

for (const mode of ['issue', 'pr'] as const) {
  test(`${mode} picker filters provider fields, bounds rows, and starts the selected chat`, async ({ mount, page }, testInfo) => {
    const candidates = Array.from({ length: 15 }, (_, index) => ({
      number: index + 1, title: `Work item ${index + 1}`, url: null, updatedAt: null,
      labels: index === 13 ? ['bugfix'] : [], headRefName: `fix/branch-${index + 1}`,
      author: 'colton', isDraft: false, reviewState: null
    }));
    let sourceAttempts = 0;
    let started: unknown;
    let created = '';
    await page.route('**/api/**', async (route) => {
      const pathname = new URL(route.request().url()).pathname;
      if (pathname.endsWith(mode === 'issue' ? '/issues' : '/prs') && ++sourceAttempts === 1) {
        await route.fulfill({ status: 503, json: { error: 'Provider temporarily unavailable' } });
        return;
      }
      let body: unknown = {};
      if (pathname.endsWith('/nodes')) body = { nodes: [] };
      if (pathname.endsWith('/settings')) body = { profileOverrides: {}, defaultBackend: 'claude' };
      if (pathname.endsWith('/models')) body = { models: [], currentModelId: null };
      if (pathname.endsWith('/issues')) body = { issues: candidates };
      if (pathname.endsWith('/prs')) body = { prs: candidates };
      if (pathname.endsWith('/start')) {
        started = route.request().postDataJSON();
        body = { session: { id: 'created' } };
      }
      await route.fulfill({ json: body });
    });
    const component = await mount(<NewChatModal open currentProfile="gah"
      profiles={[{ name: 'gah', display_name: 'GAH', repo: 'owner/gah', provider: 'github', local_path: '/tmp/gah', repo_id: 'gah', worktree_base: '/tmp/worktrees', web_url: null, max_parallel_workers: null, max_open_managed_mrs: 1, manager_wake_autonomy: null, validation_timeout_seconds: 300 }]}
      backends={[{ id: 'claude', displayName: 'Claude', implemented: true }]}
      onClose={() => {}} onCreated={(_, id) => { created = id; }} />);
    await component.getByRole('tab', { name: mode === 'issue' ? 'From issue' : 'From PR' }).click();
    const rows = component.getByRole('button', { name: /^#\d/ });
    const filter = component.getByRole('searchbox');
    await expect(component.getByRole('alert')).toContainText(`Could not load ${mode === 'issue' ? 'issues' : 'pull requests'}`);
    await expect(component.getByText(/No open .* for this project/)).not.toBeVisible();
    await expect(component.getByRole('button', { name: 'Start chat' })).toBeDisabled();
    await filter.fill('Work item');
    await component.getByRole('button', { name: mode === 'issue' ? 'Retry issues' : 'Retry pull requests' }).press('Enter');
    await expect(component.getByRole('alert')).not.toBeVisible();
    await expect(filter).toHaveValue('Work item');
    await expect(rows).toHaveCount(10);
    expect(sourceAttempts).toBe(2);
    if (mode === 'issue') await page.screenshot({ path: testInfo.outputPath('gah-1116-modal-desktop.png') });
    await component.getByRole('button', { name: 'Show all' }).click();
    await expect(rows).toHaveCount(15);
    await rows.last().click();
    await component.getByRole('button', { name: 'Show fewer' }).press('Enter');
    await expect(component.getByRole('button', { name: 'Show all' })).toBeFocused();
    await expect(rows).toHaveCount(10);
    await expect(component.getByRole('button', { name: /^#15 / })).toHaveAttribute('aria-pressed', 'true');
    await filter.fill(mode === 'issue' ? 'BUGFIX' : 'BRANCH-14');
    await expect(filter).toBeFocused();
    await expect(rows).toHaveCount(2);
    await expect(component.getByRole('button', { name: /^#14 / })).toBeVisible();
    await filter.fill('#12');
    await expect(component.getByRole('button', { name: /^#12 / })).toBeVisible();
    await filter.fill('Work item 13');
    await expect(component.getByRole('button', { name: /^#13 / })).toBeVisible();
    await filter.fill('missing');
    await expect(component.getByText(/No .* match your search/)).toBeVisible();
    await expect(rows).toHaveCount(1);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(filter).toBeVisible();
    if (mode === 'issue') await page.screenshot({ path: testInfo.outputPath('gah-1116-modal-mobile.png') });
    expect(await component.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await component.getByRole('button', { name: 'Start chat' }).click();
    expect(started).toMatchObject({ profile: 'gah', [`${mode}Number`]: 15, backend: 'claude' });
    await expect.poll(() => created).toBe('created');
  });
}


test('new chat contains keyboard focus and restores it after Escape or backdrop dismissal', async ({ mount, page }, testInfo) => {
  let closed = 0;
  await page.route('**/api/**', (route) => route.fulfill({ json: { nodes: [], profileOverrides: {}, defaultBackend: '' } }));
  const render = (open: boolean) => <div>
    <button type="button">Open chat</button>
    <NewChatModal open={open} currentProfile="gah" profiles={[]} backends={[]}
      onClose={() => { closed += 1; }} onCreated={() => {}} />
  </div>;
  const component = await mount(render(false));
  const trigger = component.getByRole('button', { name: 'Open chat', exact: true });
  await trigger.focus();
  await component.update(render(true));
  const dialog = component.getByRole('dialog', { name: 'New chat' });
  await expect(dialog).toBeVisible();
  const close = dialog.getByRole('button', { name: 'Close', exact: true });
  const cancel = dialog.getByRole('button', { name: 'Cancel', exact: true });
  await expect(close).toBeFocused();
  await cancel.focus();
  // Background controls are inert, even when focus is requested directly.
  await trigger.evaluate((button: HTMLButtonElement) => button.focus());
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(trigger).not.toBeFocused();
  // Chromium may visit browser chrome between the last and first modal control.
  if (!(await close.evaluate((button) => button === document.activeElement))) await page.keyboard.press('Tab');
  await expect(close).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(trigger).not.toBeFocused();
  if (!(await cancel.evaluate((button) => button === document.activeElement))) await page.keyboard.press('Shift+Tab');
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect.poll(() => closed).toBe(1);
  await expect(trigger).toBeFocused();
  await component.update(render(false));
  await component.update(render(true));
  await expect(dialog).toBeVisible();
  await page.mouse.click(2, 2);
  await expect(dialog).not.toBeVisible();
  await expect.poll(() => closed).toBe(2);
  await expect(trigger).toBeFocused();
});
