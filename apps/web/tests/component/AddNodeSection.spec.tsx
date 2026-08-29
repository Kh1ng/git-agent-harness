import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { AddNodeSection } from '../../src/pages/SettingsPage.js';

test('initial Add a Node DOM omits the gateway key and waits for an explicit reveal', async ({ mount, page }) => {
  const canary = 'GAH_TEST_CANARY_1014_INITIAL_DOM';
  let revealRequests = 0;

  await page.route('**/api/settings/gateway', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        url: 'http://127.0.0.1:8420',
        apiKeyConfigured: true,
        apiKey: canary,
        enabled: true,
        disabledProfiles: [],
        tailscaleIPv4: '100.64.0.42'
      })
    })
  );
  await page.route('**/api/settings/gateway/bootstrap-command', (route) => {
    revealRequests += 1;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ command: `bootstrap ${canary}` })
    });
  });

  const component = await mount(<AddNodeSection />);

  await expect(component.getByRole('heading', { name: 'Add a Node' })).toBeVisible();
  await expect(component).not.toContainText(canary);
  const revealButton = component.getByRole('button', { name: 'Reveal setup command' });
  await expect(revealButton).toBeVisible();
  expect(revealRequests).toBe(0);

  await revealButton.click();
  await expect(component).toContainText(canary);
  expect(revealRequests).toBe(1);
});
