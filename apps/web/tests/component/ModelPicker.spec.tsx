import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { Page } from '@playwright/test';
import type { ManagerBackendInfo, ManagerModelInfo } from '@git-agent-harness/contracts';
import { ProviderPicker, type ProviderSelection } from '../../src/components/ProviderPicker.js';

/** Two instances of the same provider expose identically named models:
 * only the instance label tells them apart (#1203). */
const backends: ManagerBackendInfo[] = [
  { id: 'codex-work', displayName: 'Codex · work', implemented: true },
  { id: 'codex-personal', displayName: 'Codex · personal', implemented: true },
  { id: 'hermes', displayName: 'Hermes', implemented: false }
];

const codexModels: ManagerModelInfo[] = [
  { id: 'gpt-5.3-codex', name: 'GPT-5.3 Codex', description: 'Balanced coding model' },
  { id: 'gpt-5.3-codex-mini', name: 'GPT-5.3 Codex Mini' }
];

const efforts = [
  { id: 'medium', name: 'Medium' },
  { id: 'high', name: 'High' }
];

async function stubCatalog(page: Page, failing: string[] = []) {
  await page.route('**/api/manager-chat/models*', (route) => {
    const backend = new URL(route.request().url()).searchParams.get('backend') ?? '';
    if (failing.includes(backend)) return route.fulfill({ status: 503, json: { message: 'instance offline' } });
    return route.fulfill({ json: {
      models: codexModels,
      currentModelId: 'gpt-5.3-codex',
      reasoningEfforts: efforts,
      currentReasoningEffortId: 'medium',
      contextUsage: null
    } });
  });
}

const picker = (selected: ProviderSelection[]) => (
  <div className="p-6">
    <ProviderPicker
      backends={backends}
      selectedBackendId="codex-work"
      models={codexModels}
      currentModelId="gpt-5.3-codex"
      reasoningEfforts={efforts}
      currentReasoningEffortId="medium"
      modelsLoaded
      busy={false}
      variant="session"
      catalog={{ profile: 'gah', nodeId: 'central' }}
      onSelect={(selection) => { selected.push(selection); }}
    />
  </div>
);

test('the common switch stays small and the full catalog lives one step further', async ({ mount, page }, testInfo) => {
  await stubCatalog(page);
  const selected: ProviderSelection[] = [];
  const component = await mount(picker(selected));
  const trigger = component.getByRole('button', { name: 'Provider picker' });
  await expect(trigger).toContainText('Codex · work');

  await trigger.click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });
  // The whole catalog is not rendered here: only the current selection,
  // effort, and the way into the browser.
  await expect(popover).not.toContainText('GPT-5.3 Codex Mini');
  await expect(popover.getByRole('button', { name: 'Codex · personal', exact: true })).toHaveCount(0);
  await expect(popover.getByRole('button', { name: 'High', exact: true })).toBeVisible();

  await popover.getByRole('button', { name: 'Browse all models' }).click();
  const browser = page.getByRole('dialog', { name: 'Browse models' });
  await expect(browser).toBeVisible();
  // Two live instances × (default + two models), plus the configured-but-
  // unwired provider, which is listed and unselectable rather than hidden.
  await expect(browser.getByRole('listitem')).toHaveCount(7);
  await expect(browser.getByRole('status')).toContainText('7 of 7 shown');
  await expect(browser.getByRole('button', { name: /^Hermes \(unavailable\)/ })).toBeDisabled();
  await page.screenshot({ path: testInfo.outputPath('gah-1203-browse-desktop.png') });

  const search = browser.getByRole('searchbox', { name: 'Search models' });
  await search.fill('mini');
  const cards = browser.getByRole('listitem');
  await expect(cards).toHaveCount(2);
  // Same model name on two instances: the instance label is what separates them.
  await expect(cards.filter({ hasText: 'Codex · personal' })).toHaveCount(1);
  await expect(cards.filter({ hasText: 'Codex · work' })).toHaveCount(1);

  // Search reaches the model id and the provider instance, not just names.
  await search.fill('gpt-5.3-codex-mini');
  await expect(browser.getByRole('listitem')).toHaveCount(2);
  await search.fill('personal');
  await expect(browser.getByRole('listitem')).toHaveCount(3);
  await search.fill('nothing-like-this');
  await expect(browser.getByText('No model matches that search.')).toBeVisible();
  await search.clear();
  await expect(browser.getByRole('listitem')).toHaveCount(7);

  // Provider filter chips narrow to one instance.
  await browser.getByRole('button', { name: 'Codex · personal', exact: true }).click();
  await expect(browser.getByRole('listitem')).toHaveCount(3);
  await browser.getByRole('button', { name: 'All', exact: true }).click();
  await expect(browser.getByRole('listitem')).toHaveCount(7);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(search).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('gah-1203-browse-mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1280, height: 900 });

  // Picking a model applies provider + model and closes the browser.
  const personalMini = browser.getByRole('listitem')
    .filter({ hasText: 'Codex · personal' })
    .filter({ hasText: 'GPT-5.3 Codex Mini' });
  await personalMini.getByRole('button').first().click();
  await expect(browser).not.toBeVisible();
  expect(selected.at(-1)).toEqual({ backendId: 'codex-personal', modelId: 'gpt-5.3-codex-mini', reasoningEffortId: null });
  await expect(trigger).toBeFocused();
});

test('favorites and recents use the existing storage path and apply in one click', async ({ mount, page }) => {
  await stubCatalog(page);
  const selected: ProviderSelection[] = [];
  const component = await mount(picker(selected));
  const trigger = component.getByRole('button', { name: 'Provider picker' });

  await trigger.click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });
  await popover.getByRole('button', { name: 'Browse all models' }).click();
  const browser = page.getByRole('dialog', { name: 'Browse models' });

  const star = browser.getByRole('button', { name: 'Favorite GPT-5.3 Codex Mini on Codex · personal' });
  await expect(star).toHaveAttribute('aria-pressed', 'false');
  await star.click();
  await expect(star).toHaveAttribute('aria-pressed', 'true');
  await browser.getByRole('button', { name: 'Favorites', exact: true }).click();
  await expect(browser.getByRole('listitem')).toHaveCount(1);

  // The same localStorage key the picker already used, so a reload keeps it.
  expect(await page.evaluate(() => window.localStorage.getItem('gah.composer.favorites')))
    .toBe(JSON.stringify([{ backend: 'codex-personal', model: 'gpt-5.3-codex-mini' }]));

  // Escape dismisses the browser and hands focus back to the pill.
  await page.keyboard.press('Escape');
  await expect(browser).not.toBeVisible();
  await expect(trigger).toBeFocused();

  // One click from the short popover applies the whole saved selection.
  await trigger.click();
  await expect(popover).toBeVisible();
  await popover.getByRole('button', { name: 'Apply Codex · personal · GPT-5.3 Codex Mini' }).click();
  expect(selected.at(-1)).toEqual({ backendId: 'codex-personal', modelId: 'gpt-5.3-codex-mini', reasoningEffortId: null });
  await expect(popover).not.toBeVisible();

  const recents = await page.evaluate(() => window.localStorage.getItem('gah.composer.recents'));
  expect(JSON.parse(recents ?? '[]')).toEqual([{ backend: 'codex-personal', model: 'gpt-5.3-codex-mini' }]);

  // Unstarring removes it again.
  await trigger.click();
  await popover.getByRole('button', { name: 'Remove Codex · personal · GPT-5.3 Codex Mini from favorites' }).click();
  expect(await page.evaluate(() => window.localStorage.getItem('gah.composer.favorites'))).toBe('[]');
});

test('the quick list is bounded and Escape restores trigger focus', async ({ mount, page }) => {
  await page.evaluate(() => window.localStorage.setItem('gah.composer.favorites', JSON.stringify(
    Array.from({ length: 8 }, (_, index) => ({ backend: 'codex-work', model: `model-${index}` }))
  )));
  const component = await mount(picker([]));
  const trigger = component.getByRole('button', { name: 'Provider picker' });

  await trigger.click();
  const popover = page.getByRole('dialog', { name: 'Provider picker' });
  await expect(popover.getByRole('button', { name: /^Apply / })).toHaveCount(6);
  await page.keyboard.press('Escape');
  await expect(popover).not.toBeVisible();
  await expect(trigger).toBeFocused();
});

test('an instance that does not answer keeps its default selectable and offers a retry', async ({ mount, page }) => {
  await stubCatalog(page, ['codex-personal']);
  const selected: ProviderSelection[] = [];
  const component = await mount(picker(selected));
  await component.getByRole('button', { name: 'Provider picker' }).click();
  await page.getByRole('dialog', { name: 'Provider picker' }).getByRole('button', { name: 'Browse all models' }).click();
  const browser = page.getByRole('dialog', { name: 'Browse models' });

  await expect(browser.getByRole('status')).toContainText('1 provider did not answer');
  // Three working entries, the silent instance's own default, and the
  // unwired provider.
  await expect(browser.getByRole('listitem')).toHaveCount(5);
  await expect(browser.getByRole('button', { name: /Default model\s+Codex · personal/ })).toBeVisible();
  await expect(browser.getByRole('button', { name: 'Retry', exact: true })).toBeVisible();
});
