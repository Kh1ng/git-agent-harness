import { expect, test } from '@playwright/test';
import type { PaidRouteApproval } from '@git-agent-harness/contracts';

const pending: PaidRouteApproval = { profile: 'fixture', work_id: '#822', backend: 'opencode', backend_instance: 'opencode:nous-portal-api', model: 'nous-portal/z-ai/glm-5.2', approved: false, requested: true };

test('owner reviews an exact paid route and can revoke it after granting', async ({ page }, testInfo) => {
  let row = { ...pending };
  const writes: object[] = [];
  await page.route('**/api/pairing/session', route => route.fulfill({ json: { principal: { kind: 'owner' } } }));
  await page.route('**/api/route-approvals**', async route => {
    if (route.request().method() === 'POST') {
      writes.push(route.request().postDataJSON());
      expect(route.request().headers()['idempotency-key']).toMatch(/^[a-f0-9]{32}$/);
      row = { ...row, approved: route.request().url().endsWith('/grant'), requested: route.request().url().endsWith('/revoke') };
    }
    await route.fulfill({ json: [row] });
  });
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto('/?page=work&profile=fixture');
  const approvals = page.getByRole('region', { name: 'Paid-route approvals' });
  await expect(approvals).toContainText('#822');
  await expect(approvals).toContainText('opencode:nous-portal-api');
  await approvals.getByRole('button', { name: 'Review approval' }).click();
  await expect(approvals).toContainText('does not set a spend limit');
  expect(writes).toEqual([]);
  for (const width of [320, 1280]) {
    await page.setViewportSize({ width, height: 850 });
    await approvals.scrollIntoViewIfNeeded();
    expect(await approvals.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    for (const button of await approvals.getByRole('button').all()) expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    await approvals.screenshot({ path: testInfo.outputPath(`paid-approval-${width}.png`) });
  }
  await approvals.getByRole('button', { name: 'Approve paid use' }).click();
  expect(writes).toEqual([{ profile: 'fixture', work_id: '#822', backend: 'opencode', backend_instance: 'opencode:nous-portal-api', model: 'nous-portal/z-ai/glm-5.2', confirm: true }]);
  await expect(approvals).toContainText('Approved paid use for #822.');
  await approvals.getByText('Approved routes (1)', { exact: true }).click();
  await approvals.getByRole('button', { name: 'Revoke approval' }).click();
  await approvals.getByRole('button', { name: 'Confirm revoke' }).click();
  await expect(approvals.getByRole('button', { name: 'Review approval' })).toBeVisible();
  expect(writes).toHaveLength(2);
});

test('paired devices can read requests without approving paid spend', async ({ page }) => {
  await page.route('**/api/pairing/session', route => route.fulfill({ json: { principal: { kind: 'device', id: 'phone' } } }));
  await page.route('**/api/route-approvals**', route => { expect(route.request().method()).toBe('GET'); return route.fulfill({ json: [pending] }); });
  await page.goto('/?page=work&profile=fixture');
  const approvals = page.getByRole('region', { name: 'Paid-route approvals' });
  await expect(approvals).toContainText('Owner access is required');
  await expect(approvals.getByRole('button', { name: 'Review approval' })).toHaveCount(0);
});

test('unknown mutation outcomes require refreshing before another decision', async ({ page }) => {
  let failed = false;
  await page.route('**/api/pairing/session', route => route.fulfill({ json: { principal: { kind: 'owner' } } }));
  await page.route('**/api/route-approvals**', route => {
    if (route.request().method() === 'POST') { failed = true; return route.fulfill({ status: 503, json: { message: 'Outcome unknown. Refresh status.' } }); }
    return route.fulfill({ json: [{ ...pending, approved: failed, requested: !failed }] });
  });
  await page.goto('/?page=work&profile=fixture');
  const approvals = page.getByRole('region', { name: 'Paid-route approvals' });
  await approvals.getByRole('button', { name: 'Review approval' }).click();
  await approvals.getByRole('button', { name: 'Approve paid use' }).click();
  await expect(approvals.getByRole('alert')).toContainText('Outcome unknown');
  await expect(approvals.getByRole('button', { name: 'Review approval' })).toBeDisabled();
  await approvals.getByRole('button', { name: 'Refresh approvals' }).click();
  await expect(approvals).toContainText('Approved routes (1)');
  await expect(approvals.getByRole('alert')).toHaveCount(0);
});
