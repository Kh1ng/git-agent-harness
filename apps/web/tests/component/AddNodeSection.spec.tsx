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
  const role = component.getByRole('combobox', { name: 'Install', exact: true });
  const reveal = component.getByRole('button', { name: 'Reveal Windows install command' });
  const command = component.getByRole('textbox', { name: 'Windows install command' });
  await expect(command).toHaveCount(0);
  expect(requests).toEqual([]);
  await address.fill('https://central.example.com');
  // Token was saved in the connection panel after AddNode had already mounted.
  await page.evaluate(token => sessionStorage.setItem('gah.coordinatorToken', token), token);
  for (const installRole of ['both', 'desktop', 'worker']) {
    await role.selectOption(installRole);
    await expect(command).toHaveCount(0);
    await reveal.click();
    await expect(command).toHaveValue(`install ${installRole} with ${token}`);
    expect(requests.at(-1)).toEqual({ body: { os: 'windows', centralUrl: 'https://central.example.com', role: installRole }, authorization: `Bearer ${token}` });
  }
  expect(await page.evaluate(() => sessionStorage.getItem('gah.coordinatorToken'))).toBe(token);
  expect(await page.evaluate(() => localStorage.getItem('gah.coordinatorToken'))).toBeNull();
  await page.evaluate(() => sessionStorage.removeItem('gah.coordinatorToken'));
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

for (const width of [390, 1280]) {
  test(`Unix setup selects supported roles and invalidates stale commands at ${width}px`, async ({ mount, page }) => {
    await page.setViewportSize({ width, height: 900 });
    const requests: Record<string, string>[] = [];
    await page.route('**/api/settings/nodes/command', async route => {
      const body = route.request().postDataJSON();
      requests.push(body);
      await route.fulfill({ json: { command: `install ${body.os} ${body.role}` } });
    });
    const component = await mount(<AddNodeSection />);
    await component.getByLabel('Operating system').selectOption('macos');
    await component.getByLabel('Central LAN or VPN address').fill('https://central.example.com');
    const role = component.getByRole('combobox', { name: 'Install', exact: true });
    await expect(role.locator('option')).toHaveCount(1);
    await expect(role).toHaveValue('worker');
    await component.getByRole('button', { name: 'Reveal macOS install command' }).click();
    await expect(component.getByLabel('macOS install command')).toHaveValue('install macos worker');
    expect(requests.at(-1)).toEqual({ os: 'macos', role: 'worker', centralUrl: 'https://central.example.com' });
    await component.getByLabel('Operating system').selectOption('linux');
    await expect(component.getByLabel('macOS install command')).toHaveCount(0);
    await role.selectOption('central');
    await expect(component.getByLabel('Central LAN or VPN address')).toHaveCount(0);
    await component.getByLabel('Remote memory gateway (optional)').fill('https://memory.example.com');
    await component.getByRole('button', { name: 'Reveal Linux install command' }).click();
    await expect(component.getByLabel('Linux install command')).toHaveValue('install linux central');
    expect(requests.at(-1)).toEqual({ os: 'linux', role: 'central', centralUrl: 'https://central.example.com', gatewayUrl: 'https://memory.example.com' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/gah-917-${width}.png`, fullPage: true });
  });
}
