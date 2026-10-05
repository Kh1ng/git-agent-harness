import { expect, test } from '@playwright/test';
import { readNavigation } from '../../src/lib/navigationState.js';

test('navigation links accept known pages and bounded conversation identifiers', () => {
  expect(readNavigation('?page=chat&profile=fixture&chat=mock-session-1')).toEqual({ page: 'chat', profile: 'fixture', chat: 'mock-session-1', epic: null, map: null, side: null, dock: null });
  expect(readNavigation('?page=chat&profile=gah-node%3Aworker%3Amy%2520project&chat=abc_123')).toEqual({ page: 'chat', profile: 'gah-node:worker:my%20project', chat: 'abc_123', epic: null, map: null, side: null, dock: null });
  expect(readNavigation('?page=unknown&profile=%0Asecret&chat=abc')).toEqual({ page: 'overview', profile: null, chat: null, epic: null, map: null, side: null, dock: null });
  // Sidebar views: `side` names one, and older links that named it as the page still open it.
  expect(readNavigation('?page=git&side=settings')).toMatchObject({ page: 'git', side: 'settings' });
  expect(readNavigation('?page=events')).toMatchObject({ page: 'overview', side: 'events' });
  expect(readNavigation('?side=git').side).toBeNull();
  // The chat docks beside a main page; it cannot dock beside itself.
  expect(readNavigation('?page=git&dock=chat').dock).toBe('chat');
  expect(readNavigation('?page=chat&dock=chat').dock).toBeNull();
  expect(readNavigation('?page=chat&profile=fixture&chat=../../other').chat).toBeNull();
  expect(readNavigation(`?profile=${'a'.repeat(513)}&chat=abc`).profile).toBeNull();
  expect(readNavigation(`?profile=fixture&chat=${'a'.repeat(129)}`).chat).toBeNull();
  expect(readNavigation('?chat=abc').chat).toBeNull();
  expect(readNavigation('?page=planning&profile=fixture&epic=900')).toEqual({ page: 'planning', profile: 'fixture', chat: null, epic: '900', map: null, side: null, dock: null });
  for (const epic of ['0', '-1', '9x', '1'.repeat(11)]) expect(readNavigation(`?profile=fixture&epic=${epic}`).epic).toBeNull();
  expect(readNavigation('?epic=900').epic).toBeNull();
  expect(readNavigation('?profile=fixture&map=node-handoff').map).toBe('node-handoff');
  for (const map of ['../etc', 'Bad', '-x', 'a/b']) expect(readNavigation(`?profile=fixture&map=${encodeURIComponent(map)}`).map).toBeNull();
});

test('a mobile conversation link restores its project and chat after reload and page navigation', async ({ page, request }) => {
  const mock = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';
  expect((await request.post(`${mock}/api/mock/scenario`, { data: { name: 'normal' } })).ok()).toBe(true);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/?page=chat&profile=fixture&chat=mock-session-1#keep-marker');
  const chatSelect = page.getByRole('combobox', { name: 'Chat', exact: true });
  await expect(chatSelect).toHaveValue('mock-session-1');
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex');
  expect(new URL(page.url()).hash).toBe('#keep-marker');
  await page.reload();
  await expect(chatSelect).toHaveValue('mock-session-1');
  await page.getByRole('button', { name: 'Open navigation menu' }).click();
  await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Fleet', exact: true }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('page')).toBe('nodes');
  await page.getByRole('button', { name: 'Open navigation menu' }).click();
  await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(chatSelect).toHaveValue('mock-session-1');

  // Selecting the default conversation drops the session id and keeps the
  // rest of the URL.
  await chatSelect.selectOption('');
  await expect.poll(() => new URL(page.url()).searchParams.get('chat')).toBeNull();
  expect(new URL(page.url()).searchParams.get('profile')).toBe('fixture');
  expect(new URL(page.url()).hash).toBe('#keep-marker');
  await page.reload();
  await expect(chatSelect).toHaveValue('');

  // The Projects page links back to the project's default conversation.
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(chatSelect).toHaveValue('');
});
