import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const MOCK_BASE_URL = process.env.GAH_MOCK_BASE_URL ?? 'http://127.0.0.1:3774';

// Coverage for the project-grouped session picker: sessions of every
// project render under their project (collapsible, persisted), settled and
// archived sessions sort by project under an Archive section, and picking
// another project's session moves the whole chat page to that project.

async function selectScenario(request: APIRequestContext, name: string): Promise<void> {
  const response = await request.post(`${MOCK_BASE_URL}/api/mock/scenario`, { data: { name } });
  expect(response.ok(), await response.text()).toBe(true);
}

async function openChat(page: Page): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByPlaceholder(/Message the manager/)).toBeVisible();
}

async function openSessionPicker(page: Page): Promise<void> {
  await page.getByLabel('Chat session').click();
  await expect(page.getByRole('listbox', { name: 'All sessions' })).toBeVisible();
}

function pickerSection(page: Page, name: 'Active' | 'Archive') {
  return page.getByRole('listbox', { name: 'All sessions' }).getByRole('group', { name });
}

test('sessions group under their project and archived ones sort by project under Archive', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await openSessionPicker(page);

  // Both projects render as groups: the fixture profile by its display
  // name, the second project (absent from /api/profiles) by its raw id.
  const active = pickerSection(page, 'Active');
  await expect(active.getByRole('group', { name: 'Fixture', exact: true })).toBeVisible();
  await expect(active.getByRole('group', { name: 'fixture-two', exact: true })).toBeVisible();
  await expect(active.getByRole('option', { name: 'Mock session', exact: true })).toBeVisible();
  await expect(active.getByRole('option', { name: 'Second project session', exact: true })).toBeVisible();

  // Settled/archived sessions live only under Archive, projects sorted.
  const archive = pickerSection(page, 'Archive');
  await expect(archive.getByRole('option', { name: 'Settled mock session', exact: true })).toBeVisible();
  await expect(archive.getByRole('option', { name: 'Archived second session', exact: true })).toBeVisible();
  // Projects appear in sorted order (the headers are uppercased by CSS, so
  // read the group labels instead of the rendered text).
  const archiveProjects = await archive.getByRole('group').evaluateAll((groups) =>
    groups.map((group) => group.getAttribute('aria-label')).filter((label) => label && label !== 'Archive'));
  expect(archiveProjects).toEqual(['Fixture', 'fixture-two']);
  await expect(archive.getByRole('option', { name: 'Mock session', exact: true })).toHaveCount(0);
});

test('collapsing a project group persists across a reload', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await openSessionPicker(page);

  const fixtureTwo = pickerSection(page, 'Active').getByRole('group', { name: 'fixture-two', exact: true });
  const header = fixtureTwo.getByRole('button', { name: 'fixture-two', exact: true });
  await expect(header).toHaveAttribute('aria-expanded', 'true');
  await header.click();
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  await expect(fixtureTwo.getByRole('option', { name: 'Second project session', exact: true })).toHaveCount(0);

  await page.reload();
  await openChat(page);
  await openSessionPicker(page);
  const headerAfterReload = pickerSection(page, 'Active')
    .getByRole('group', { name: 'fixture-two', exact: true })
    .getByRole('button', { name: 'fixture-two', exact: true });
  await expect(headerAfterReload).toHaveAttribute('aria-expanded', 'false');
  await expect(pickerSection(page, 'Active').getByRole('option', { name: 'Second project session', exact: true })).toHaveCount(0);
  // The untouched project stayed expanded.
  await expect(pickerSection(page, 'Active').getByRole('option', { name: 'Mock session', exact: true })).toBeVisible();
});

test('selecting a cross-project session switches the chat page to that project', async ({ page, request }) => {
  await selectScenario(request, 'normal');
  await openChat(page);
  await openSessionPicker(page);

  await page.getByRole('option', { name: 'Second project session', exact: true }).click();

  // The picker closes and shows the picked session; the page header now
  // describes the second project (its raw profile id — the mock's profile
  // list doesn't cover it), and the composer pill reflects that session's
  // own provider once its model list loads (the mock advertises codex for
  // every project).
  const trigger = page.getByLabel('Chat session');
  await expect(trigger).toContainText('Second project session');
  await expect(page.getByText('fixture-two', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Provider picker' })).toContainText('Codex', { timeout: 10_000 });
});
