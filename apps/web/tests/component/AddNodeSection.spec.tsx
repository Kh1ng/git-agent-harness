import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { AddNodeSection } from '../../src/pages/SettingsPage.js';

test('Windows setup reveals the selected role explicitly and sends only the tab token', async ({ mount, page }) => {
  const token = 'GAH_TEST_SETUP_TOKEN';
  const requests: { body: unknown; authorization?: string }[] = [];
  await page.route('**/api/settings/nodes/command', async (route) => {
    const body = route.request().postDataJSON();
    requests.push({ body, authorization: route.request().headers().authorization });
    await route.fulfill({ json: { command: `install ${body.role} with ${token}` } });
  });
  const component = await mount(<AddNodeSection />);
  const address = component.getByLabel('Central LAN or VPN address');
  const credential = component.getByLabel('Central access token', { exact: false });
  const role = component.getByRole('combobox', { name: 'Install', exact: true });
  const reveal = component.getByRole('button', { name: 'Reveal Windows install command' });
  const command = component.getByRole('textbox', { name: 'Windows install command' });
  await expect(command).toHaveCount(0);
  expect(requests).toEqual([]);
  await address.fill('https://central.example.com');
  await credential.fill(token);
  await expect(credential).toHaveAttribute('type', 'password');
  expect(await page.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBeNull();
  for (const installRole of ['both', 'desktop', 'worker']) {
    await role.selectOption(installRole);
    await expect(command).toHaveCount(0);
    await reveal.click();
    await expect(command).toHaveValue(`install ${installRole} with ${token}`);
    expect(requests.at(-1)).toEqual({ body: { centralUrl: 'https://central.example.com', role: installRole }, authorization: `Bearer ${token}` });
  }
  expect(await page.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBe(token);
  expect(await page.evaluate(() => localStorage.getItem('gah.coordinatorToken'))).toBeNull();
  await credential.clear();
  await expect(command).toHaveCount(0);
  await reveal.click();
  await expect(command).toBeVisible();
  expect(requests.at(-1)?.authorization).toBeUndefined();
  expect(await page.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBeNull();
});

test('pending setup locks its inputs; server and clipboard failures leave a recoverable command', async ({ mount, page }) => {
  let release: () => void = () => {};
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let attempts = 0;
  await page.route('**/api/settings/nodes/command', async (route) => {
    attempts++;
    if (attempts === 1) {
      await pending;
      await route.fulfill({ status: 503, json: { message: 'Windows installer unavailable' } });
    } else {
      await route.fulfill({ json: { command: 'install-worker' } });
    }
  });
  await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', {
    configurable: true, value: { writeText: async () => { throw new DOMException('Permission denied', 'NotAllowedError'); } }
  }));
  const component = await mount(<AddNodeSection />);
  const reveal = component.getByRole('button', { name: 'Reveal Windows install command' });
  await reveal.click();
  await expect(component.getByRole('button', { name: 'Preparing…' })).toBeDisabled();
  await expect(component.getByLabel('Central LAN or VPN address')).toBeDisabled();
  await expect(component.getByRole('combobox', { name: 'Install', exact: true })).toBeDisabled();
  await expect(component.getByLabel('Central access token', { exact: false })).toBeDisabled();
  expect(attempts).toBe(1);
  release();
  await expect(component.getByRole('alert')).toHaveText('Windows installer unavailable');
  await expect(reveal).toBeEnabled();
  await reveal.click();
  const command = component.getByRole('textbox', { name: 'Windows install command' });
  await expect(command).toHaveValue('install-worker');
  await expect(component.getByRole('alert')).toHaveCount(0);
  await component.getByRole('button', { name: 'Copy command', exact: true }).click();
  await expect(component.getByRole('alert')).toContainText('Select the command above and copy it manually');
  await command.focus();
  expect(await command.evaluate((element: HTMLTextAreaElement) => element.value.slice(element.selectionStart, element.selectionEnd))).toBe('install-worker');
  expect(attempts).toBe(2);
});
