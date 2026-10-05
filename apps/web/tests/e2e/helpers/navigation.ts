import { expect, type Page } from '@playwright/test';

/** The expanded chat page: its rail lists projects and conversations. */
export async function openProjects(page: Page): Promise<void> {
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
}

/** Usage is a navbar dropdown on desktop; the phone drawer lists its pages one by one. */
export async function openUsage(page: Page, name: 'Telemetry' | 'Quota'): Promise<void> {
  const drawer = page.getByRole('button', { name: 'Open navigation menu' });
  if (await drawer.isVisible()) {
    await drawer.click();
    await page.getByRole('dialog', { name: 'Navigation menu' }).getByRole('button', { name, exact: true }).click();
    return;
  }
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: 'Usage', exact: true }).click();
  await page.getByRole('menu', { name: 'Usage' }).getByRole('menuitem', { name, exact: true }).click();
}
