import { expect, test } from '@playwright/test';

test('an expired login is a red row naming node, backend, and provider (#1271)', async ({ page }) => {
  await page.route('**/api/auth-health', (route) => route.fulfill({ json: { rows: [
    { node_id: 'mac', node_name: 'Mac', backend: 'opencode', provider: 'github-copilot', state: 'expired', source: 'probe',
      detail: 'A credential is saved but the provider lists no models.', since: new Date(Date.now() - 5 * 60_000).toISOString() },
    { node_id: 'central', node_name: 'Central', backend: 'claude', provider: null, state: 'ok', source: 'probe' }
  ] } }));
  await page.goto('/?page=nodes');
  const logins = page.getByRole('region', { name: 'Logins' });
  await expect(logins.getByText('Mac · opencode · github-copilot')).toBeVisible();
  await expect(logins.getByText(/Login expired · detected/)).toBeVisible();
  await expect(logins.getByText('A credential is saved but the provider lists no models.')).toBeVisible();
  await expect(logins.getByText('Central · claude')).toHaveCount(0);
});
