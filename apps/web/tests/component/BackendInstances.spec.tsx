import { expect, test } from '@playwright/experimental-ct-react';
import { BackendInstancesCard } from '../../src/pages/SettingsPage.js';

test('connected-node instance creation supports all runners without exposing local keys', async ({ mount, page }) => {
  let added: unknown;
  await page.route('**/api/backend-instances**', route => {
    if (route.request().method() === 'POST') added = route.request().postDataJSON();
    return route.fulfill({ json: { profile: 'central-profile', backend_instances: [] } });
  });
  const component = await mount(<BackendInstancesCard profileName="central-profile" effective={{ backend_instances: [] }} />);
  await expect(component.getByText(/on the connected node/)).toBeVisible();
  await component.getByRole('button', { name: 'Add account', exact: true }).click();
  await expect(component.getByLabel('Runner').getByRole('option')).toHaveText(['Codex', 'Claude', 'OpenCode', 'Vibe', 'OpenHands', 'Hermes', 'Antigravity']);
  await component.getByLabel('Runner').selectOption('vibe');
  await component.getByLabel('Account label').fill('Vibe second');
  await component.getByLabel('Instance ID').fill('vibe-second');
  await component.getByRole('button', { name: 'Add isolated account' }).click();
  await expect(component.getByRole('button', { name: 'Add account', exact: true })).toBeVisible();
  expect(added).toEqual({ profile: 'central-profile', instance: 'vibe-second', runnerKind: 'vibe', accountLabel: 'Vibe second' });
  await expect(component.locator('input[type="password"]')).toHaveCount(0);
});
