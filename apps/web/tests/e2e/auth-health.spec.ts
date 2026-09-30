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

test('Fix login shows the sign-in link and one-time code, then the result (#1272)', async ({ page }) => {
  const row = { node_id: 'mac', node_name: 'Mac', backend: 'opencode', provider: 'github-copilot', state: 'expired', source: 'probe', since: new Date().toISOString() };
  await page.route('**/api/auth-health', (route) => route.fulfill({ json: { rows: [row] } }));
  const base = { id: 'r1', node_id: 'mac', backend: 'opencode', provider: 'github-copilot', expires_at: new Date(Date.now() + 600_000).toISOString() };
  let started: unknown;
  let keySeen = '';
  let polls = 0;
  await page.route('**/api/auth-health/repairs', (route) => {
    started = route.request().postDataJSON();
    return route.fulfill({ status: 201, json: { key: 'k'.repeat(64), repair: { ...base, status: 'starting' } } });
  });
  await page.route('**/api/auth-health/repairs/r1', (route) => {
    keySeen = route.request().headers()['x-login-repair-key'] ?? '';
    polls++;
    return route.fulfill({ json: polls < 2
      ? { ...base, status: 'open_url', url: 'https://github.com/login/device', code: 'WDJB-MJHT' }
      : { ...base, status: 'succeeded' } });
  });
  await page.goto('/?page=nodes');
  const logins = page.getByRole('region', { name: 'Logins' });
  await logins.getByRole('button', { name: 'Fix login' }).click();
  expect(started).toEqual({ node_id: 'mac', backend: 'opencode', provider: 'github-copilot' });
  await expect(logins.getByLabel('One-time code')).toHaveText('WDJB-MJHT');
  await expect(logins.getByRole('link', { name: 'Open sign-in page' })).toHaveAttribute('href', 'https://github.com/login/device');
  expect(keySeen).toBe('k'.repeat(64));
  await expect(logins.getByText(/Logged in again/)).toBeVisible({ timeout: 10_000 });
});
