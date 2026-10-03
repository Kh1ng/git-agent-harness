import { test, expect } from '@playwright/experimental-ct-react';
import { LoginRepairPanel } from '../../src/components/LoginRepairPanel.js';

for (const [backend, provider, label, url] of [
  ['gh', 'github', 'GitHub CLI', 'https://cli.github.com/'],
  ['glab', 'gitlab', 'GitLab CLI', 'https://gitlab.com/gitlab-org/cli#installation'],
]) {
  test(`${label} installation precedes authentication`, async ({ mount, page }) => {
    await page.route('**/api/auth-health/repairs', route => route.fulfill({
      json: { key: 'test-key', repair: { id: 'repair-1', node_id: 'central', backend, provider,
        expires_at: new Date(Date.now() + 60_000).toISOString(), status: 'install_required', install_url: url } },
    }));
    const component = await mount(<LoginRepairPanel login={{ node_id: 'central', node_name: 'This computer', backend, provider, installed: false }} />);
    await expect(component.getByRole('link', { name: `Install ${label}` })).toHaveAttribute('href', url);
    await expect(component.getByRole('button', { name: 'Fix login' })).toHaveCount(0);
    await component.getByRole('button', { name: 'Check installation' }).click();
    await expect(component.getByText('Sign-in starts only after the CLI is available.', { exact: false })).toBeVisible();
    await expect(component.getByRole('link', { name: 'Open sign-in page' })).toHaveCount(0);
  });
}
