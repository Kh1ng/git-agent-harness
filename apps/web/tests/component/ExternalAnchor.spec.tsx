import { test, expect } from '@playwright/experimental-ct-react';
import { ExternalAnchor } from '../../src/components/ExternalAnchor.js';

// The component-test harness reuses an already-loaded page, so host state
// is installed and reset via evaluate (init scripts never run), and every
// test clears whatever the previous one left behind.

type HostState = { open?: unknown; webkit?: unknown; desktop?: boolean };

async function installHost(page: import('@playwright/test').Page, host: HostState) {
  await page.evaluate(({ open, webkit, desktop }: HostState) => {
    const w = window as unknown as Record<string, unknown> & {
      open: (url?: string | URL, target?: string, features?: string) => Window | null;
    };
    delete w.webkit;
    delete w.__GAH_DESKTOP_EXTERNAL_LINKS__;
    (w as unknown as { __externalOpens?: string[] }).__externalOpens = [];
    w.open = (url, target, features) => {
      (w.__externalOpens as string[]).push(`${String(url)}|${String(target)}|${String(features)}`);
      return null;
    };
    if (open === 'throw') {
      w.open = () => {
        throw new Error('window.open must not run in a mobile shell');
      };
    }
    if (webkit) w.webkit = webkit;
    if (desktop) w.__GAH_DESKTOP_EXTERNAL_LINKS__ = true;
    (w as unknown as { __defaultPrevented?: boolean | null }).__defaultPrevented = null;
    const marked = w as unknown as { __clickRecorderInstalled?: boolean };
    if (!marked.__clickRecorderInstalled) {
      marked.__clickRecorderInstalled = true;
      document.addEventListener(
        'click',
        (event) => {
          setTimeout(() => {
            (w as unknown as { __defaultPrevented?: boolean | null }).__defaultPrevented = event.defaultPrevented;
          });
        },
        { capture: true }
      );
    }
  }, host);
}

const defaultPrevented = (element: HTMLElement) =>
  (element.ownerDocument.defaultView as unknown as { __defaultPrevented?: boolean | null }).__defaultPrevented;
const externalOpens = (element: HTMLElement) =>
  (element.ownerDocument.defaultView as unknown as { __externalOpens?: string[] }).__externalOpens;

test.describe('ExternalAnchor', () => {
  test('renders a plain anchor with hardening rel and no target', async ({ mount }) => {
    const component = await mount(
      <ExternalAnchor href="https://github.com/owner/repo/pull/12">View PR</ExternalAnchor>
    );
    await expect(component).toHaveAttribute('href', 'https://github.com/owner/repo/pull/12');
    await expect(component).toHaveAttribute('rel', 'noopener noreferrer');
    await expect(component).not.toHaveAttribute('target');
  });

  test('browser host: a plain click is handled with a noopener tab open', async ({ mount, page }) => {
    await installHost(page, {});
    const component = await mount(
      <ExternalAnchor href="https://github.com/owner/repo/pull/12">View PR</ExternalAnchor>
    );
    await component.click();
    await expect.poll(() => component.evaluate(externalOpens)).toEqual([
      'https://github.com/owner/repo/pull/12|_blank|noopener,noreferrer',
    ]);
    await expect.poll(() => component.evaluate(defaultPrevented)).toEqual(true);
  });

  test('ios shell host: a plain click keeps the default anchor navigation', async ({ mount, page }) => {
    await installHost(page, { open: 'throw', webkit: { messageHandlers: { gahController: {} } } });
    // Passthrough means the browser's default anchor navigation must run;
    // intercept the external origin so the harness page survives to see it.
    await page.route('https://github.com/**', (route) => route.fulfill({ body: 'external-ok' }));
    const component = await mount(
      <ExternalAnchor href="https://github.com/owner/repo/pull/12">View PR</ExternalAnchor>
    );
    await component.click();
    await page.waitForURL('https://github.com/**');
  });

  test('desktop host: a plain click is cancelled for the browser and delegated to the shell bridge', async ({ mount, page }) => {
    await installHost(page, { desktop: true });
    const component = await mount(
      <ExternalAnchor href="https://github.com/owner/repo/pull/12">View PR</ExternalAnchor>
    );
    await component.click();
    await expect.poll(() => component.evaluate(defaultPrevented)).toEqual(true);
    // The shell bridge is unavailable in the test host, so the component's
    // fallback opened a plain browser tab rather than a dead link.
    await expect.poll(() => component.evaluate(externalOpens)).toEqual([
      'https://github.com/owner/repo/pull/12|_blank|noopener,noreferrer',
    ]);
  });

  test('modified clicks fall through to the browser default', async ({ mount, page }) => {
    await installHost(page, {});
    const component = await mount(
      <ExternalAnchor href="https://github.com/owner/repo/pull/12">View PR</ExternalAnchor>
    );
    // Shift is the portable modified-click modifier; Meta is not delivered
    // reliably by the Linux CI runners, and Control is the macOS context-menu gesture.
    await component.click({ modifiers: ['Shift'] });
    await expect.poll(() => component.evaluate(defaultPrevented)).toEqual(false);
    await expect.poll(() => component.evaluate(externalOpens)).toEqual([]);
  });
});
