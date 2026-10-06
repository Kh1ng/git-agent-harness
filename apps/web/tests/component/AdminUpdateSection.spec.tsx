import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';
import { test, expect } from '@playwright/experimental-ct-react';
import { AdminUpdateSection } from '../../src/pages/SettingsPage.js';
import React from 'react';

test.beforeEach(async ({ page }) => { await page.routeWebSocket('**/ws*', socket => socket.close()); });

test.describe('AdminUpdateSection', () => {
  test('renders nothing when the server reports admin update disabled', async ({ mount, page }) => {
    await page.route('**/api/admin/update', (route) =>
      route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ message: 'disabled' }) })
    );
    await page.route('**/api/admin/update/status', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ status: 'idle' }) })
    );
    await page.route('**/api/admin/release/status', (route) =>
      route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ message: 'disabled' }) })
    );

    const component = await mount(<WebSocketProvider><AdminUpdateSection /></WebSocketProvider>);
    await expect(component).toBeEmpty();
  });

  test('clicking Update and restart starts a release install and renders live progress', async ({ mount, page }) => {
    // First status poll (on mount) reports idle; every poll after the click
    // reports running, so the test can assert on the in-progress state
    // without racing the 2s re-poll back to a terminal status.
    let statusCalls = 0;
    await page.route('**/api/admin/update/status', (route) => {
      statusCalls += 1;
      const running = statusCalls > 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: running ? 'running' : 'idle',
          startedAt: running ? new Date().toISOString() : null,
          finishedAt: null,
          exitCode: null,
          pid: running ? 4242 : null,
          output: running ? 'Updating GAH CLI/control plane...' : ''
        })
      });
    });
    await page.route('**/api/admin/release/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          channel: 'edge',
          current_version: '0.1.3',
          latest_version: '0.1.4',
          update_available: true,
          release_url: 'https://github.com/Kh1ng/git-agent-harness/releases/tag/edge',
          published_at: '2026-10-05T00:00:00.000Z',
          notes: '## 0.1.4 (edge)\n\n- Release channel and in-app updates (#1416).'
        })
      })
    );
    let requestedMode = '';
    await page.route('**/api/admin/update', (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            current: { hash: 'aaa', short: 'aaa', subject: 'old' },
            latest: { hash: 'bbb', short: 'bbb', subject: 'new' },
            commitsBehind: 2,
            upToDate: false
          })
        });
      }
      requestedMode = (route.request().postDataJSON() as { mode?: string }).mode ?? '';
      return route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'running',
          startedAt: new Date().toISOString(),
          finishedAt: null,
          exitCode: null,
          pid: 4242,
          output: 'Updating GAH CLI/control plane...',
          mode: 'release'
        })
      });
    });

    const component = await mount(<WebSocketProvider><AdminUpdateSection /></WebSocketProvider>);
    await expect(component.getByText('2 commit(s) behind: aaa → bbb')).toBeVisible();
    await expect(component.getByText(/edge channel · this server runs v0\.1\.3 · latest published is v0\.1\.4/)).toBeVisible();

    await component.getByRole('button', { name: 'Update and restart (v0.1.4)' }).click();

    // The primary button installs release artifacts (--from-release), not a
    // source rebuild (issue #1416).
    await expect.poll(() => requestedMode).toBe('release');
    await expect(component.getByRole('button', { name: 'Updating…' })).toBeDisabled();
    await expect(component.getByText(/Status: running/)).toBeVisible();
    await expect(component.getByText('Updating GAH CLI/control plane...')).toBeVisible();
  });

  test('Rebuild from source keeps the developer path', async ({ mount, page }) => {
    let statusCalls = 0;
    await page.route('**/api/admin/update', (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ current: null, latest: null, commitsBehind: 0, upToDate: true })
        });
      }
      return route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({ status: 'running', output: '', mode: 'source' })
      });
    });
    await page.route('**/api/admin/update/status', (route) => {
      statusCalls += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ status: statusCalls > 1 ? 'running' : 'idle', output: '' })
      });
    });
    await page.route('**/api/admin/release/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          channel: 'edge',
          current_version: '0.1.3',
          latest_version: '0.1.3',
          update_available: false,
          release_url: null,
          published_at: null,
          notes: ''
        })
      })
    );

    const component = await mount(<WebSocketProvider><AdminUpdateSection /></WebSocketProvider>);
    await component.getByRole('button', { name: 'Rebuild from source' }).click();
    await expect(component.getByText(/Status: running/)).toBeVisible();
  });
});
