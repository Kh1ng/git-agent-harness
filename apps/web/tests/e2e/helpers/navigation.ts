import { expect, type Page } from '@playwright/test';

export async function openProjects(page: Page): Promise<void> {
  await page.goto('/?page=projects&profile=fixture');
  await expect(page.getByRole('complementary', { name: 'Chat navigation' })).toBeVisible();
}
