import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { WorkerSnapshotProbe } from '../../src/test-utils/WorkerSnapshotProbe.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

for (const fails of [false, true]) {
  test(`roster pushes survive an in-flight status ${fails ? 'failure' : 'response'}`, async ({ mount, page }) => {
    await page.evaluate(() => {
      class Socket {
        static OPEN = 1;
        readyState = 1;
        onopen: (() => void) | null = null;
        onmessage: ((event: { data: string }) => void) | null = null;
        constructor() {
          (window as unknown as { rosterSocket: Socket }).rosterSocket = this;
          setTimeout(() => this.onopen?.(), 0);
        }
        send() {}
        close() {}
      }
      window.WebSocket = Socket as unknown as typeof WebSocket;
    });
    let release!: () => void;
    const waiting = new Promise<void>(resolve => { release = resolve; });
    await page.route('**/api/status?**', async route => {
      await waiting;
      await route.fulfill({ status: fails ? 500 : 200, json: fails
        ? { error: 'status failed' }
        : { profile: { profile: 'gah' }, running_workers: [{ run_id: 'old-http' }] } });
    });
    const component = await mount(<WebSocketProvider><WorkerSnapshotProbe /></WebSocketProvider>);
    await component.getByRole('button', { name: 'Fetch status' }).click();
    await expect(component).toContainText('Loading');
    const push = async (profile: string, runId: string) => page.evaluate(({ profile, runId }) => {
      (window as unknown as { rosterSocket: { onmessage: (event: { data: string }) => void } }).rosterSocket.onmessage({
        data: JSON.stringify({ type: 'workers.snapshot', profile, workers: [{ run_id: runId }] }),
      });
    }, { profile, runId });
    await push('other', 'wrong-profile');
    await expect(component).not.toContainText('wrong-profile');
    await push('gah', 'new-push');
    await expect(component).toContainText('Workers: new-push');
    await expect(component).toContainText('Loading');
    release();
    await expect(component).toContainText('Settled');
    await expect(component).toContainText('Workers: new-push');
    await expect(component).not.toContainText('old-http');
  });
}
