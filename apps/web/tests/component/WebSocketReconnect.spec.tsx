import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { WebSocketConnectionProbe } from '../../src/test-utils/WebSocketConnectionProbe.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

test('a transient websocket failure reconnects and clears the error', async ({ mount, page }) => {
  await page.evaluate(() => {
    const testWindow = window as typeof window & { __wsAttempts: number; __wsCloses: number };
    testWindow.__wsAttempts = 0;
    testWindow.__wsCloses = 0;

    class ReconnectingWebSocket {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;

      readonly url: string;
      readyState = ReconnectingWebSocket.CONNECTING;
      onopen: ((event: Event) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      onmessage: ((event: MessageEvent) => void) | null = null;

      constructor(url: string | URL) {
        this.url = String(url);
        testWindow.__wsAttempts += 1;
        const attempt = testWindow.__wsAttempts;
        window.setTimeout(() => {
          if (attempt === 1 || attempt === 3) {
            this.readyState = ReconnectingWebSocket.CLOSED;
            this.onerror?.(new Event('error'));
            this.onclose?.(new CloseEvent('close'));
          } else {
            this.readyState = ReconnectingWebSocket.OPEN;
            this.onopen?.(new Event('open'));
          }
        }, 0);
      }

      send() {}

      close() {
        testWindow.__wsCloses += 1;
        this.readyState = ReconnectingWebSocket.CLOSED;
        this.onclose?.(new CloseEvent('close'));
      }
    }

    window.WebSocket = ReconnectingWebSocket as unknown as typeof WebSocket;
  });

  const component = await mount(
    <WebSocketProvider>
      <WebSocketConnectionProbe />
    </WebSocketProvider>
  );

  await expect(component.getByText('Connection error: WebSocket connection error')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as typeof window & { __wsAttempts: number }).__wsAttempts), {
    timeout: 6_000,
  }).toBe(2);
  await expect(component.getByText('Live')).toBeVisible();

  await component.getByRole('button', { name: 'Replace connection' }).click();
  await expect(component.getByText('Connection error: WebSocket connection error')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as typeof window & { __wsAttempts: number }).__wsAttempts), {
    timeout: 6_000,
  }).toBe(4);
  await expect(component.getByText('Live')).toBeVisible();

  await component.unmount();
  await expect.poll(() => page.evaluate(() => (window as typeof window & { __wsCloses: number }).__wsCloses)).toBe(2);
});
