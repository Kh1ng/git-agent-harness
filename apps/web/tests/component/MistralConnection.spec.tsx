import { expect, test } from '@playwright/experimental-ct-react';
import { MistralConnectionPanel } from '../../src/components/MistralConnectionPanel.js';
import { NativeMistralConnection } from './fixtures/NativeMistralConnection.js';

test('native quota entry uses the existing local Settings bridge; browser offers desktop guidance', async ({ mount, page }) => {
  await page.evaluate(() => { window.__GAH_DESKTOP_MISTRAL_LOGIN__ = true; });
  const native = await mount(<MistralConnectionPanel />);
  await expect(native.getByRole('link', { name: 'Open this computer’s Settings' })).toHaveAttribute('href', 'gah://settings');
  await native.unmount();
  await page.evaluate(() => { delete window.__GAH_DESKTOP_MISTRAL_LOGIN__; });
  const browser = await mount(<MistralConnectionPanel />);
  await expect(browser.getByText(/Open GAH’s desktop app/)).toBeVisible();
  await expect(browser.getByRole('link')).toHaveCount(0);
  await expect(browser.getByRole('textbox')).toHaveCount(0);
});

test('local Settings signs in, checks explicitly, and confirms this device without exposing credentials', async ({ mount, page }, testInfo) => {
  const component = await mount(<NativeMistralConnection responses={[
    { state: 'pending', installed: true, message: 'Sign in in the Mistral window, then check the connection.' },
    { state: 'pending', installed: true, message: 'Finish signing in before checking again.' },
    { state: 'connected', installed: true, message: 'Mistral usage is connected on this computer.' },
  ]} />);
  await expect(component.getByRole('button', { name: 'Check connection' })).toBeHidden();
  await component.getByRole('button', { name: 'Connect Mistral', exact: true }).click();
  await expect(component.getByRole('status')).toHaveText(/Sign in in the Mistral window/);
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 844 });
    await expect(component.getByRole('button', { name: 'Check connection' })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`mistral-connect-${width}.png`) });
  }
  await component.getByRole('button', { name: 'Check connection' }).click();
  await expect(component.getByRole('status')).toHaveText('Finish signing in before checking again.');
  await component.getByRole('button', { name: 'Check connection' }).click();
  await expect(component.getByRole('status')).toHaveText('Mistral usage is connected on this computer.');
  await expect(component.getByRole('button', { name: 'Reconnect Mistral' })).toBeVisible();
  await expect(component.getByRole('button', { name: 'Check connection' })).toBeHidden();
  await expect(component.getByRole('list', { name: 'Native commands' }).getByRole('listitem')).toHaveText(['mistral_login_start', 'mistral_login_finish', 'mistral_login_finish']);
  await expect(component.getByRole('textbox')).toHaveCount(0);
});

test('cancelled, missing CLI, and native errors remain actionable without reporting a connection', async ({ mount }) => {
  const component = await mount(<NativeMistralConnection responses={[
    { state: 'cancelled', installed: true, message: 'The Mistral sign-in window was closed.' },
    { state: 'unavailable', installed: false, message: 'GAH CLI was not found on this computer.' },
    null,
  ]} />);
  const start = component.getByRole('button', { name: 'Connect Mistral', exact: true });
  await start.click();
  await expect(component.getByRole('status')).toHaveText('The Mistral sign-in window was closed.');
  await expect(component.getByRole('button', { name: 'Check connection' })).toBeHidden();
  await start.click();
  await expect(component.getByText(/GAH CLI is unavailable on this computer/)).toBeVisible();
  await expect(component.getByRole('button', { name: 'Check connection' })).toBeHidden();
  await start.click();
  await expect(component.getByRole('status')).toHaveText(/Could not connect Mistral/);
  await expect(component.getByText(/Private native diagnostic/)).toHaveCount(0);
  await expect(start).toBeEnabled();
});

test('automatic native verification wins over an earlier pending command response', async ({ mount }) => {
  const component = await mount(<NativeMistralConnection
    responses={[{ state: 'pending', installed: true, message: 'Sign in to Mistral.' }]}
    automatic={{ state: 'connected', installed: true, message: 'Mistral usage is connected on this computer.' }}
  />);
  await component.getByRole('button', { name: 'Connect Mistral', exact: true }).click();
  await expect(component.getByRole('status')).toHaveText('Mistral usage is connected on this computer.');
  await expect(component.getByRole('button', { name: 'Reconnect Mistral' })).toBeEnabled();
  await expect(component.getByRole('button', { name: 'Check connection' })).toBeHidden();
});
