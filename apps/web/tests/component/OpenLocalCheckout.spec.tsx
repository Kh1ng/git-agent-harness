import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { OpenLocalCheckout } from '../../src/components/OpenLocalCheckout.js';

test('browsers receive no native launch action', async ({ mount }) => {
  const component = await mount(<OpenLocalCheckout profile="gah" nodeName="central node" />);
  await expect(component.getByRole('button')).toHaveCount(0);
});

test('the desktop split button remembers a bounded installed tool and sends no path', async ({ mount, page }) => {
  await page.evaluate(() => {
    window.__GAH_DESKTOP_OPEN_PROJECT__ = true;
    (window as unknown as { openCalls: unknown[] }).openCalls = [];
    (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__ = {
      invoke: async (command, args) => {
        (window as unknown as { openCalls: unknown[] }).openCalls.push({ command, args });
        if (command === 'desktop_open_context') return {
          available: true,
          preferredTool: 'file_manager',
          tools: [{ id: 'file_manager', label: 'Finder' }, { id: 'vscode', label: 'VS Code' }],
          reason: null
        };
      }
    };
  });
  const component = await mount(<OpenLocalCheckout profile="gah" nodeId="mac-1" nodeName="MacBook" sessionId="session-1" />);
  const primary = component.getByRole('button', { name: 'Open in Finder' });
  await expect(primary).toBeVisible();
  const menuButton = component.getByRole('button', { name: 'Choose local app' });
  await menuButton.click();
  await component.getByRole('button', { name: 'VS Code' }).click();
  await expect(menuButton).toBeFocused();
  await expect(component.getByRole('button', { name: 'Open in VS Code' })).toBeVisible();
  await component.getByRole('button', { name: 'Open in VS Code' }).click();

  const calls = await page.evaluate(() => (window as unknown as { openCalls: Array<{ command: string; args: unknown }> }).openCalls);
  expect(calls).toEqual([
    { command: 'desktop_open_context', args: { project: { profile: 'gah', nodeId: 'mac-1', sessionId: 'session-1' } } },
    { command: 'open_local_checkout', args: { project: { profile: 'gah', nodeId: 'mac-1', sessionId: 'session-1' }, tool: 'vscode' } },
    { command: 'open_local_checkout', args: { project: { profile: 'gah', nodeId: 'mac-1', sessionId: 'session-1' }, tool: 'vscode' } }
  ]);
  expect(JSON.stringify(calls)).not.toContain('/Users/');
});

test('a desktop checkout owned elsewhere names its node without offering a launch', async ({ mount, page }) => {
  await page.evaluate(() => {
    window.__GAH_DESKTOP_OPEN_PROJECT__ = true;
    (window as unknown as { __TAURI_INTERNALS__: { invoke: () => Promise<unknown> } }).__TAURI_INTERNALS__ = {
      invoke: async () => ({ available: false, preferredTool: null, tools: [], reason: 'This checkout belongs to another device.' })
    };
  });
  const component = await mount(<OpenLocalCheckout profile="gah" nodeId="windows-1" nodeName="Windows worker" />);
  await expect(component.getByText('Files on Windows worker')).toBeVisible();
  await expect(component.getByRole('button')).toHaveCount(0);
});
