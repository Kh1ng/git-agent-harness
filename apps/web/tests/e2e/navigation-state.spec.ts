import { expect, test } from '@playwright/test';
import { readNavigation } from '../../src/lib/navigationState.js';

test('navigation links accept known pages and bounded conversation identifiers', () => {
  expect(readNavigation('?page=chat&profile=fixture&chat=mock-session-1')).toEqual({ page: 'chat', profile: 'fixture', chat: 'mock-session-1' });
  expect(readNavigation('?page=chat&profile=gah-node%3Aworker%3Amy%2520project&chat=abc_123')).toEqual({ page: 'chat', profile: 'gah-node:worker:my%20project', chat: 'abc_123' });
  expect(readNavigation('?page=unknown&profile=%0Asecret&chat=abc')).toEqual({ page: 'overview', profile: null, chat: null });
  expect(readNavigation('?page=chat&profile=fixture&chat=../../other').chat).toBeNull();
  expect(readNavigation(`?profile=${'a'.repeat(513)}&chat=abc`).profile).toBeNull();
  expect(readNavigation(`?profile=fixture&chat=${'a'.repeat(129)}`).chat).toBeNull();
  expect(readNavigation('?chat=abc').chat).toBeNull();
});

test('a mobile conversation link restores its project and chat after reload and page navigation', async ({ page, request }) => {
  const mock = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';
  expect((await request.post(`${mock}/api/mock/scenario`, { data: { name: 'normal' } })).ok()).toBe(true);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/?page=chat&profile=fixture&chat=mock-session-1#keep-marker');
  const title = page.getByText('Mock session', { exact: true });
  await expect(title).toBeVisible();
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex · GPT-5.3 Codex');
  expect(new URL(page.url()).hash).toBe('#keep-marker');
  await page.reload();
  await expect(title).toBeVisible();
  await page.getByRole('button', { name: 'Open navigation menu' }).click();
  await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Nodes', exact: true }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('page')).toBe('nodes');
  await page.getByRole('button', { name: 'Open navigation menu' }).click();
  await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(title).toBeVisible();

  // A blank chat drops the conversation from the link and keeps everything
  // else the URL was carrying.
  await page.getByRole('button', { name: 'New chat', exact: true }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('chat')).toBeNull();
  expect(new URL(page.url()).searchParams.get('profile')).toBe('fixture');
  expect(new URL(page.url()).hash).toBe('#keep-marker');
  await page.reload();
  await expect(page.getByRole('heading', { name: 'New chat', exact: true })).toBeVisible();

  // The Projects page links back to the project's default conversation.
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(page.getByText('Default conversation', { exact: true })).toBeVisible();
});
