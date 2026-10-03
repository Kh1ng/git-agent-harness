import { expect, test } from '@playwright/experimental-ct-react';
import { NativeProviderConnections } from './fixtures/NativeProviderConnections.js';

const entries = [
  { id: 'work-key', provider: 'mistral', kind: 'api_key' as const, account_label: 'Work', env_var: null },
  { id: 'personal-key', provider: 'mistral', kind: 'api_key' as const, account_label: 'Personal', env_var: null },
];

test('multiple named keys save through masked transient input and bind only to a selected local instance', async ({ mount, page }, testInfo) => {
  const component = await mount(<NativeProviderConnections entries={entries} />);
  await expect(component.getByText('Work · mistral · API key', { exact: true })).toBeVisible();
  await expect(component.getByText('Personal · mistral · API key', { exact: true })).toBeVisible();
  const work = component.locator('[data-credential-id="work-key"]');
  await expect(work.getByRole('button', { name: 'Use on instance' })).toBeDisabled();
  await work.getByRole('combobox').selectOption({ label: 'local-work / vibe-work · vibe' });
  await work.getByRole('button', { name: 'Use on instance' }).click();
  await expect(component.getByRole('status')).toHaveText(/Work is selected for local-work \/ vibe-work on this computer/);
  await expect(work.getByRole('option', { name: /agy-one/ })).toHaveCount(0);
  await component.getByRole('button', { name: 'Add API key', exact: true }).click();
  await component.getByLabel('Connection name', { exact: true }).fill('Nous work');
  await component.getByLabel('Provider', { exact: true }).fill('nous');
  const key = component.getByLabel('API key', { exact: true });
  await expect(key).toHaveAttribute('type', 'password');
  await key.fill('synthetic-private-api-key');
  await component.getByRole('button', { name: 'Save key', exact: true }).click();
  await expect(key).toHaveValue('');
  await expect(component.getByText('Nous work · nous · API key')).toBeVisible();
  await expect(component.locator('#native-calls')).toContainText('"secretMatched":true');
  await expect(component.locator('#native-calls')).not.toContainText('synthetic-private-api-key');
  await expect(component.getByRole('status')).toHaveText(/Usage has not been checked/);
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`provider-connections-${width}.png`) });
  }
});

test('two separate Mistral sign-ins keep selected slot IDs and names independent', async ({ mount }) => {
  const component = await mount(<NativeProviderConnections entries={[]} />);
  await component.getByLabel('Another Mistral account name').fill('Mistral work');
  await component.getByRole('button', { name: 'Connect another Mistral account' }).click();
  await component.getByLabel('Another Mistral account name').fill('Mistral personal');
  await component.getByRole('button', { name: 'Connect another Mistral account' }).click();
  await expect(component.getByRole('button', { name: 'Check connection', exact: true })).toHaveCount(2);
  await component.getByRole('button', { name: 'Check connection', exact: true }).first().click();
  await expect(component.getByText('Mistral work · mistral · Mistral sign-in')).toBeVisible();
  await expect(component.getByText('Mistral personal · Mistral sign-in in progress')).toBeVisible();
  const calls = JSON.parse(await component.locator('#native-calls').textContent() ?? '[]') as { command: string; id: string }[];
  const starts = calls.filter(call => call.command === 'mistral_login_start');
  expect(starts).toHaveLength(2);
  expect(starts[0].id).not.toBe(starts[1].id);
  expect(calls.find(call => call.command === 'mistral_login_finish')?.id).toBe(starts[0].id);
});

test('confirmed removal selects one credential; usage failure preserves inventory and stays generic', async ({ mount }) => {
  const component = await mount(<NativeProviderConnections entries={entries} failUsage />);
  const work = component.locator('[data-credential-id="work-key"]');
  await work.getByRole('button', { name: 'Check usage' }).click();
  await expect(component.getByRole('status')).toHaveText(/Usage is unavailable for Work/);
  await expect(work.getByText('Saved on this computer · Usage check failed')).toBeVisible();
  await expect(component).not.toContainText('synthetic-private-api-key');
  await expect(component.getByText('Personal · mistral · API key')).toBeVisible();
  await work.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(work.getByRole('button', { name: 'Remove connection', exact: true })).toBeVisible();
  await expect(component.locator('#native-calls')).not.toContainText('credential_remove');
  await work.getByRole('button', { name: 'Remove connection', exact: true }).click();
  await expect(work).toHaveCount(0);
  await expect(component.getByText('Personal · mistral · API key')).toBeVisible();
});

test('a saved key can explicitly create a compatible local instance without a terminal command', async ({ mount, page }) => {
  const component = await mount(<NativeProviderConnections entries={entries} />);
  const work = component.locator('[data-credential-id="work-key"]');
  await work.getByRole('button', { name: 'Create local instance' }).click();
  await expect(work.getByLabel('Local runner')).toHaveValue('vibe');
  await work.getByLabel('Local instance ID').fill('vibe-work-two');
  await page.setViewportSize({ width: 390, height: 1000 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await work.getByRole('button', { name: 'Create and use key' }).click();
  await expect(component.getByRole('status')).toHaveText(/local-work \/ vibe-work-two was created using Work on this computer/);
  await expect(component.locator('#native-calls')).toContainText('credential_add_instance');
  await expect(component.locator('#native-calls')).toContainText('work-key');
  await work.getByRole('button', { name: 'Check usage' }).click();
  await expect(work.getByText('Saved on this computer · Usage check completed')).toBeVisible();
});

test('failed metadata reload preserves previous named connections', async ({ mount }) => {
  const component = await mount(<NativeProviderConnections entries={entries} failReload />);
  await expect(component.getByText('Work · mistral · API key')).toBeVisible();
  await component.getByRole('button', { name: 'Check saved connections' }).click();
  await expect(component.getByText('Work · mistral · API key')).toBeVisible();
  await expect(component.getByRole('status')).toHaveText(/connection could not be updated/);
});

test('automatic success cannot be overwritten by an older pending slot response', async ({ mount }) => {
  const component = await mount(<NativeProviderConnections entries={[]} automatic />);
  await component.getByLabel('Another Mistral account name').fill('Automatic Mistral');
  await component.getByRole('button', { name: 'Connect another Mistral account' }).click();
  await expect(component.getByText('Automatic Mistral · mistral · Mistral sign-in')).toBeVisible();
  await expect(component.getByRole('button', { name: 'Check connection', exact: true })).toHaveCount(0);
  await expect(component.getByRole('status')).not.toHaveText('Sign in to this account.');
});
