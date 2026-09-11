import { test, expect } from '@playwright/experimental-ct-react';
import { PwaStatusBars } from '../../src/components/PwaStatusBars.js';
import React from 'react';

// Issue #534: the PWA status bars render only when relevant — an offline
// state shows the stale/read-only banner; a pending service-worker update
// shows the reload prompt. Neither appears in the default (online,
// up-to-date) state.

test('renders the offline banner when the browser reports offline', async ({ mount, page }) => {
  const component = await mount(<PwaStatusBars />);
  // Drive the state through the same event the browser fires on connectivity
  // loss — more faithful than mocking navigator.onLine.
  await page.evaluate(() => window.dispatchEvent(new Event('offline')));
  await expect(component).toContainText('Offline');
  await expect(component).toContainText('read-only');
});

test('renders nothing in the default online state', async ({ mount }) => {
  const component = await mount(<PwaStatusBars />);
  await expect(component).not.toContainText('Offline');
  await expect(component).not.toContainText('Reload to update');
});
