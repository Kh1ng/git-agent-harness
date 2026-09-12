import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { Navbar } from '../../src/components/Navbar.js';
import { SessionDetailModal } from '../../src/components/SessionDetailModal.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

test.use({ hasTouch: true });

test('mobile navigation contains focus, closes with Escape, selection, backdrop, and desktop resize', async ({ mount, page }, testInfo) => {
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 390, height: 844 });
  const selected: string[] = [];
  await mount(<Navbar currentPage="overview" onPageChange={(value) => selected.push(value)} />);
  const opener = page.getByRole('button', { name: 'Open navigation menu' });
  const dialog = page.getByRole('dialog', { name: 'Navigation menu' });
  await opener.focus();
  await page.keyboard.press('Enter');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Close navigation menu' })).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  // Browsers may include their chrome in the tab cycle, but the page stays inert.
  await expect(opener).not.toBeFocused();
  await page.keyboard.press('Tab');
  await expect(dialog.getByRole('button', { name: 'Close navigation menu' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(opener).toBeFocused();
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
  const focus = await opener.evaluate((button) => ({ style: getComputedStyle(button).outlineStyle, width: button.getBoundingClientRect().width, height: button.getBoundingClientRect().height }));
  expect(focus.style).toBe('solid');
  expect(focus.width).toBeGreaterThanOrEqual(44);
  expect(focus.height).toBeGreaterThanOrEqual(44);
  await opener.click();
  await dialog.getByRole('button', { name: 'Factory', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  expect(selected).toEqual(['work']);
  await opener.click();
  await page.mouse.click(380, 400);
  await expect(dialog).not.toBeVisible();
  await opener.click();
  for (const button of await dialog.getByRole('navigation').getByRole('button').all()) {
    expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  }
  await page.screenshot({ path: testInfo.outputPath('gah-audit-mobile-dark.png') });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect(dialog).not.toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('gah-audit-desktop-dark.png') });
});

test('session controls stay reachable on small phones, landscape, and desktop', async ({ mount, page }, testInfo) => {
  const sent: { type: string; command?: string; sessionId?: string }[] = [];
  const session = { id: 'session-very-long-identifier-without-breaks-012345678901234567890', providerKind: 'claude' as const, status: 'running' as const, repo: 'owner/repository-with-a-long-name', mode: 'improve', target: '#1112', backend: 'claude', branch: 'feature/mobile-controller-with-a-long-branch-name', model: 'claude-model-long-identifier', startedAt: '2026-09-08T10:00:00.000Z' };
  await page.routeWebSocket('**/ws', ws => {
    ws.onMessage(raw => {
      const message = JSON.parse(String(raw));
      sent.push(message);
      if (message.type === 'client.hello') {
        ws.send(JSON.stringify({ type: 'server.welcome', serverVersion: 'test', serverProviderCatalog: { providers: [] }, sessions: [session], providers: {} }));
        ws.send(JSON.stringify({ type: 'session.stdout', sessionId: session.id, data: 'Long output line '.repeat(50) }));
      }
    });
  });
  await page.reload();
  await mount(<WebSocketProvider><SessionDetailModal session={session} onClose={() => {}} /></WebSocketProvider>);
  const modal = page.getByRole('dialog', { name: 'Session: Improve #1112' });
  await modal.getByRole('textbox', { name: 'Session command' }).fill('show status');
  for (const viewport of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1440, height: 900 }]) {
    await page.setViewportSize(viewport);
    await expect(modal).toBeVisible();
    expect(await modal.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    for (const control of await modal.locator('button, input').all()) {
      const bounds = (await control.boundingBox())!;
      expect(bounds.height).toBeGreaterThanOrEqual(44);
      expect(bounds.y).toBeGreaterThanOrEqual(0);
      expect(bounds.y + bounds.height).toBeLessThanOrEqual(viewport.height);
    }
    await expect(modal.getByRole('textbox', { name: 'Session command' })).toHaveCSS('font-size', '16px');
    await expect(modal.getByRole('textbox', { name: 'Session command' })).toHaveValue('show status');
    await page.screenshot({ path: testInfo.outputPath(`session-${viewport.width}x${viewport.height}.png`) });
  }
  await page.setViewportSize({ width: 320, height: 568 });
  await modal.getByRole('button', { name: 'Send', exact: true }).click();
  await expect.poll(() => sent.some(message => message.type === 'session.sendCommand' && message.command === 'show status' && message.sessionId === session.id)).toBe(true);
  await modal.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect.poll(() => sent.some(message => message.type === 'session.stop' && message.sessionId === session.id)).toBe(true);
});

test('semantic text and badges retain contrast in explicit and system themes', async ({ mount, page }, testInfo) => {
  await mount(<div className="bg-card p-4"><p className="text-muted">Last seen</p><button className="btn-primary">Add node</button>{['good', 'warning', 'serious', 'critical'].map((status) => <span key={status} className={`badge badge-${status}`}>{status}</span>)}</div>);
  for (const theme of ['dark', 'light', 'system']) {
    await page.emulateMedia({ colorScheme: theme === 'dark' ? 'dark' : 'light' });
    await page.evaluate((value) => { if (value === 'system') delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = value; }, theme);
    const ratios = await page.evaluate(() => {
      const rgb = (value: string) => value.match(/[\d.]+/g)!.map(Number);
      const luminance = (color: number[]) => color.slice(0, 3).map((c) => c / 255).map((c) => c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4).reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
      return [...document.querySelectorAll('.text-muted,.btn-primary,.badge')].map((element) => {
        const style = getComputedStyle(element);
        const background = rgb(style.backgroundColor);
        const parent = rgb(getComputedStyle(element.parentElement!).backgroundColor);
        const alpha = background[3] ?? 1;
        const blended = background.slice(0, 3).map((c, i) => c * alpha + parent[i] * (1 - alpha));
        const fg = luminance(rgb(style.color));
        const bg = luminance(blended);
        return { text: element.textContent, ratio: (Math.max(fg, bg) + 0.05) / (Math.min(fg, bg) + 0.05) };
      });
    });
    for (const { text, ratio } of ratios) expect(ratio, `${theme}: ${text}`).toBeGreaterThanOrEqual(4.5);
  }
  await page.screenshot({ path: testInfo.outputPath('gah-audit-light-tokens.png') });
});


test('session details use a modal with a named command input and Escape dismissal', async ({ mount, page }, testInfo) => {
  await page.routeWebSocket('**/ws', () => {});
  let closed = false;
  await mount(<React.StrictMode><WebSocketProvider><SessionDetailModal session={{ id: 'test-session', providerKind: 'claude', status: 'running', repo: 'owner/repo', mode: 'improve', target: '#1112', backend: 'claude' }} onClose={() => { closed = true; }} /></WebSocketProvider></React.StrictMode>);
  const modal = page.getByRole('dialog', { name: 'Session: Improve #1112' });
  await expect(modal).toBeVisible();
  expect(closed).toBe(false);
  await expect(modal.getByRole('textbox', { name: 'Session command' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(modal).not.toBeVisible();
  await expect.poll(() => closed).toBe(true);
});
