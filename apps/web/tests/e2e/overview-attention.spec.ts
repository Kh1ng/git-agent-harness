import { expect, test } from '@playwright/test';
import { attentionRows } from '../../src/components/AttentionTable.js';

// Overview's Needs attention is one table, ten rows a page, sortable by
// kind, work id or summary; a blocked work item opens to its plan.

// Routes that fetch from the fixture server must not outlive the test.
test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test('attention rows unify profile blockers, blocked work, dependencies and review holds', () => {
  const rows = attentionRows({
    blockers: [{ kind: 'sync_failed', message: 'git fetch failed' }],
    blockedWorkItems: [{ kind: 'human_required', source_reference: '#946', reason_code: 'retry_budget_exhausted', message: 'three attempts failed' }],
    dependencyBlockers: [{ ticket_path: 't', work_id: '#950', title: 'Add caching', reason_code: 'deps', reason: 'waiting', dependencies: [{ identity: '#940', normalized_state: 'open' }, { identity: '#930', normalized_state: 'closed' }] }] as never,
    reviewHeldWorkIds: ['#960']
  });
  expect(rows.map((row) => [row.kind, row.work, row.summary])).toEqual([
    ['profile', 'sync failed', 'git fetch failed'],
    ['work', '#946', 'retry budget exhausted: three attempts failed'],
    ['dependency', '#950', 'Add caching · waiting · blocked on #940 [open]'],
    ['review', '#960', 'Manager review hold active']
  ]);
});

test('Needs attention pages ten rows at a time and sorts by column', async ({ page }) => {
  await page.route('**/api/status**', async (route) => {
    const snapshot = await (await route.fetch()).json();
    snapshot.blockers = [];
    snapshot.dependency_blockers = [];
    snapshot.review_held_work_ids = ['#2'];
    snapshot.blocked_work_items = Array.from({ length: 11 }, (_, i) => ({
      kind: 'human_required', source_reference: `#${100 + i}`, reason_code: 'retry_budget_exhausted', message: `attempt ${i} failed`,
      remediation_plan: { result: 'plan', reason_code: 'retry_budget_exhausted', required_authority: 'operator', safe_actions: [{ summary: `Clear attempts on #${100 + i}`, command: `gah ledger clear-attempts --work-id ${100 + i}` }] }
    }));
    await route.fulfill({ json: snapshot });
  });
  await page.goto('/?page=overview&profile=fixture');
  const table = page.getByRole('region', { name: /Needs attention/ });
  const rows = table.locator('tbody tr');
  await expect(rows).toHaveCount(10);
  await expect(table.getByText('1–10 of 12')).toBeVisible();
  // Blocked work sorts before the review hold; the hold is on the second page.
  await expect(rows.first()).toContainText('#100');
  await expect(table.getByText('Review hold', { exact: true })).toHaveCount(0);
  await table.getByRole('button', { name: 'Next page' }).click();
  await expect(rows).toHaveCount(2);
  await expect(table.getByText('Review hold', { exact: true })).toBeVisible();
  await expect(table.getByText('11–12 of 12')).toBeVisible();
  // Sorting by work id resets to the first page, descending puts the highest id first.
  await table.getByRole('button', { name: 'Work' }).click();
  await expect(rows.first()).toContainText('#2');
  await table.getByRole('button', { name: 'Work' }).click();
  await expect(rows.first()).toContainText('#110');
  // A blocked row opens to its remediation plan.
  await rows.first().click();
  await expect(table.getByText('Clear attempts on #110')).toBeVisible();
  await expect(table.getByRole('button', { name: 'Clear attempts & retry' })).toBeVisible();
  // The work id opens its detail in the left sidebar, not a drawer over the page.
  await rows.first().getByRole('button', { name: '#110' }).click();
  const sidebar = page.getByRole('complementary', { name: 'Git issues' });
  await expect(sidebar.getByRole('button', { name: 'Back to the list' })).toBeVisible();
  await expect(sidebar).toContainText('#110');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(new URL(page.url()).searchParams.get('side')).toBe('issues');
});
