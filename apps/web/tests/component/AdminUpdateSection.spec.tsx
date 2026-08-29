import { test, expect } from '@playwright/experimental-ct-react';
import { AdminUpdateSection } from '../../src/pages/SettingsPage.js';
import React from 'react';

test.describe('AdminUpdateSection', () => {
  test('renders nothing when the server reports admin update disabled', async ({ mount, page }) => {
    await page.route('**/api/admin/update', (route) =>
      route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ message: 'disabled' }) })
    );
    await page.route('**/api/admin/update/status', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ status: 'idle' }) })
    );

    const component = await mount(<AdminUpdateSection />);
    await expect(component).toBeEmpty();
  });

  test('clicking Update now starts an update and renders live progress', async ({ mount, page }) => {
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
      return route.fulfill({
        status: 202,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'running',
          startedAt: new Date().toISOString(),
          finishedAt: null,
          exitCode: null,
          pid: 4242,
          output: 'Updating GAH CLI/control plane...'
        })
      });
    });

    const component = await mount(<AdminUpdateSection />);
    await expect(component.getByText('2 commit(s) behind: aaa → bbb')).toBeVisible();

    const button = component.getByRole('button', { name: 'Update now' });
    await button.click();

    await expect(component.getByRole('button', { name: 'Updating…' })).toBeDisabled();
    await expect(component.getByText('Status: running')).toBeVisible();
    await expect(component.getByText('Updating GAH CLI/control plane...')).toBeVisible();
  });
});
