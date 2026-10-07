import { test, expect } from '@playwright/experimental-ct-react';
import React from 'react';
import { DispatchSettingsSection } from '../../src/pages/ProfilePanel.js';
import { GlobalManagerSection, NotificationChannelSection } from '../../src/pages/SettingsPage.js';

// Issue #1465: a label beside its control, with no htmlFor, leaves the control
// without an accessible name. These mount the real page sections and query by
// role and name, which only resolves when label and control are tied together.

test('Profile dispatch settings fields are named by their labels', async ({ mount }) => {
  const component = await mount(
    <DispatchSettingsSection
      selectedName="demo"
      selected={{ max_parallel_workers: 2, validation_timeout_seconds: 300, manager_wake_autonomy: 'review_only' }}
      profileLoading={false}
      profileError={null}
    />
  );
  await expect(component.getByRole('spinbutton', { name: 'Max parallel workers' })).toHaveValue('2');
  await expect(component.getByRole('spinbutton', { name: 'Validation command timeout (seconds)' })).toHaveValue('300');
  await expect(component.getByRole('combobox', { name: 'Manager wake autonomy' })).toHaveValue('review_only');
  await expect(component.getByRole('checkbox', { name: 'Hold schema and API contract changes for my review' })).toBeChecked();
});

test('Settings global manager and notification fields are named by their labels', async ({ mount }) => {
  const config = {
    data: { current_manager: 'codex', notifications: { channel: 'telegram' as const, telegram_chat_id: '123456789' } },
    loading: false,
    error: null,
  };
  const setConfig = async () => {};
  const clearConfigErrors = () => {};
  const component = await mount(
    <>
      <GlobalManagerSection config={config} setConfig={setConfig} clearConfigErrors={clearConfigErrors} />
      <NotificationChannelSection config={config} setConfig={setConfig} clearConfigErrors={clearConfigErrors} />
    </>
  );
  await expect(component.getByRole('textbox', { name: 'Current manager' })).toHaveValue('codex');
  await expect(component.getByRole('combobox', { name: 'Channel' })).toHaveValue('telegram');
  await expect(component.getByRole('textbox', { name: 'Telegram chat ID' })).toHaveValue('123456789');
});
