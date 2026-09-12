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
      backend_configured: { codex: true },
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

test('review holds are visible on Overview and do not hide ticket status in Factory', async ({ page }) => {
  await serveHeldStatus(page);
  await page.route('**/api/manager-chat/settings', (route) => route.fulfill({ json: {
    defaultBackend: 'codex', profileOverrides: {}, availableBackends: [
      { id: 'codex', displayName: 'Codex', implemented: true },
      { id: 'hermes', displayName: 'Hermes', implemented: true },
    ],
  } }));
  await page.route('**/api/report**', (route) => route.fulfill({ json: { comparisons: [] } }));
  await page.route('**/api/usage/rollup**', (route) => route.fulfill({ json: { tickets: [] } }));
  await page.route('**/api/pairing/session', (route) => route.fulfill({ json: { principal: { kind: 'owner' } } }));
  await page.route('**/api/route-approvals**', (route) => route.fulfill({ json: [{
    profile: 'gah', work_id: '#641', backend: 'codex', backend_instance: null, model: null, approved: false, requested: true,
  }] }));
  await page.route('**/api/external-approvals**', (route) => route.fulfill({ json: [{
    profile: 'gah', repo_id: 'repo', work_id: '#642', credential_label: 'test', operation_kind: 'read', state: 'requested', active: false,
    allowed_env_vars: [], max_requests: 1, max_dollars: null, expires_at: null, purpose: 'Test', consumed_requests: 0, consumed_dollars: null, denial_reason: null,
  }] }));
  await page.goto('/');

  await expect(page.getByText(`Manager review hold active on ${heldWorkId}`)).toBeVisible();

  await page.getByRole('button', { name: 'Factory', exact: true }).click();
  const backendPicker = page.getByRole('button', { name: 'Factory backend' });
  await expect(backendPicker).toBeVisible();
  await backendPicker.click();
  const picker = page.getByRole('dialog', { name: 'Factory backend' });
  await expect(picker.getByRole('button', { name: 'Codex', exact: true })).toBeEnabled();
  await expect(picker.getByRole('button', { name: /Hermes/ })).toBeDisabled();
  await expect(picker.getByText('Model', { exact: true })).toHaveCount(0);
  await expect(page.locator('dt', { hasText: 'Needs review' }).locator('..').locator('dd')).toHaveText('3');
  const reviewQueue = page.getByRole('list', { name: 'Review queue' });
  await expect(reviewQueue).toContainText('#640');
  await expect(reviewQueue).toContainText('#641');
  await expect(reviewQueue).toContainText('#642');
  const ticketRow = page.getByRole('row').filter({ hasText: 'Display active review holds' });
  await expect(ticketRow.getByText('Review hold', { exact: true })).toBeVisible();
  await expect(ticketRow.getByText('Human required', { exact: true })).toBeVisible();
});
