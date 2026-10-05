import { expect, type Page } from '@playwright/test';

/** The expanded chat page: its rail lists projects and conversations. */
export async function openProjects(page: Page): Promise<void> {
  await page.goto('/?page=chat&profile=fixture&chat=default');
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
}
