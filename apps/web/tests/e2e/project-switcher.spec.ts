import { expect, test } from '@playwright/test';

// "Project: <name>" at the far left of the navbar switches the whole
// dashboard between configured projects and reaches the import and
// create forms.

// Routes that fetch from the fixture server must not outlive the test.
test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test.beforeEach(async ({ page }) => {
  await page.route('**/api/profiles', async (route) => {
    const profiles = await (await route.fetch()).json();
    await route.fulfill({ json: [...profiles, { ...profiles[0], name: 'second', display_name: 'Second', repo: 'owner/second' }] });
  });
});

test('the switcher changes the project for every page and keeps it in the URL', async ({ page }) => {
  await page.goto('/?page=overview');
  const switcher = page.getByRole('button', { name: /^Project:/ });
  await expect(switcher).toHaveText(/Project: Fixture/);
  await switcher.click();
  const menu = page.getByRole('menu', { name: 'Projects' });
  await expect(menu.getByRole('menuitemradio', { name: /^Fixture/ })).toHaveAttribute('aria-checked', 'true');
  await menu.getByRole('menuitemradio', { name: /^Second/ }).click();
  await expect(switcher).toHaveText(/Project: Second/);
  await expect.poll(() => new URL(page.url()).searchParams.get('profile')).toBe('second');
  await expect(menu).toHaveCount(0);
  // The Profile sidebar follows the switcher rather than carrying its own selector.
  await page.getByRole('button', { name: 'Profile', exact: true }).click();
  const current = page.getByRole('region', { name: 'Current project' });
  await expect(current).toContainText('Second');
  await expect(current.getByRole('combobox')).toHaveCount(0);
});

test('Import from Git opens the Projects import form and Create new opens the add profile form', async ({ page }) => {
  await page.goto('/?page=overview');
  await page.getByRole('button', { name: /^Project:/ }).click();
  await page.getByRole('menuitem', { name: 'Import from Git' }).click();
  await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible();
  const rail = page.getByRole('complementary', { name: 'Chat navigation' });
  await expect(rail.locator('details').filter({ hasText: 'Import from Git' })).toHaveAttribute('open', '');
  await expect(rail.getByRole('button', { name: 'Import repository' })).toBeVisible();

  await page.getByRole('button', { name: /^Project:/ }).click();
  await page.getByRole('menuitem', { name: 'Create new' }).click();
  const profilePanel = page.getByRole('complementary', { name: 'Profile' });
  await expect(profilePanel.getByRole('heading', { name: 'Profile Management' })).toBeVisible();
  await expect(profilePanel.getByRole('heading', { name: 'Add New Profile' })).toBeVisible();
  await expect(profilePanel.getByPlaceholder('/path/to/local/repo')).toBeVisible();
});
