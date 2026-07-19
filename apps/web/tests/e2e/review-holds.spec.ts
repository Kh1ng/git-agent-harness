import { expect, test, type Page } from '@playwright/test';

const heldWorkId = '#640';

async function serveHeldStatus(page: Page) {
  await page.route('**/api/status**', (route) => route.fulfill({
    json: {
      profile: { display_name: 'GAH' },
      blockers: [],
      blocked_work_items: [],
      review_held_work_ids: [heldWorkId],
      merge_requests: [],
      recent_ledger: null,
      active_claims: [],
      issue_intake_rejections: [],
      available_tickets: [{
        ticket_path: 'github:#640',
        work_id: heldWorkId,
        title: 'Display active review holds',
        prior_attempt_count: 0,
        human_required: true,
        has_active_claim: false,
        has_active_mr: true,
        recommended_backend: null,
        recommended_model: null,
        last_failure_class: null,
      }],
    },
  }));
  await page.route('**/api/quota**', (route) => route.fulfill({
    json: { candidates: [], usage: null },
  }));
  await page.route('**/api/profiles**', (route) => route.fulfill({ json: [] }));
  await page.route('**/api/controller-activity**', (route) => route.fulfill({ json: [] }));
  await page.route('**/api/loop/status**', (route) => route.fulfill({
    json: { running: false },
  }));
}

test('review holds are visible on Overview and do not hide ticket status on Work', async ({ page }) => {
  await serveHeldStatus(page);
  await page.goto('/');

  await expect(page.getByText(`Manager review hold active on ${heldWorkId}`)).toBeVisible();

  await page.getByRole('button', { name: 'Work', exact: true }).click();
  const ticketRow = page.getByRole('row').filter({ hasText: 'Display active review holds' });
  await expect(ticketRow.getByText('Review hold', { exact: true })).toBeVisible();
  await expect(ticketRow.getByText('Human required', { exact: true })).toBeVisible();
});
