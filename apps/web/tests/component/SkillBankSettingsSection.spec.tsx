import { test, expect } from '@playwright/experimental-ct-react';
import { SkillBankSettingsSection } from '../../src/pages/SettingsPage.js';
import React from 'react';

const SKILL_MD = `---
id: gah-manager
version: 2.0.0
displayName: GAH Manager
description: Orchestrates GAH work.
backends: [hermes, codex]
---

# Role: GAH Manager
`;

test.describe('SkillBankSettingsSection', () => {
  test('renders the read-only inventory from GET /api/skills', async ({ mount, page }) => {
    await page.route('**/api/skills', (route) => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          skills: [
            { id: 'gah-manager', version: '1.0.0', displayName: 'GAH Manager', description: 'desc', backends: [], source: 'docs', bound: true }
          ]
        })
      });
    });

    const component = await mount(<SkillBankSettingsSection />);
    await expect(component.getByText('gah-manager@1.0.0')).toBeVisible();
    await expect(component.getByRole('button', { name: 'Upload SKILL.md' })).toBeVisible();
  });

  test('uploading a SKILL.md with front matter posts the parsed skill and refreshes the inventory', async ({ mount, page }) => {
    let listCalls = 0;
    let postedBody: Record<string, unknown> | null = null;

    await page.route('**/api/skills', (route) => {
      if (route.request().method() === 'GET') {
        listCalls += 1;
        const skills = postedBody
          ? [{ id: postedBody.id, version: postedBody.version, displayName: postedBody.displayName, description: postedBody.description, backends: postedBody.backends, source: postedBody.source, bound: false }]
          : [];
        return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ skills }) });
      }
      if (route.request().method() === 'POST') {
        postedBody = route.request().postDataJSON();
        return route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({ ...postedBody, createdAt: 1, updatedAt: 1 })
        });
      }
      return route.continue();
    });

    const component = await mount(<SkillBankSettingsSection />);
    await expect(component.getByText('No skills installed')).toBeVisible();

    await component.locator('input[type="file"]').setInputFiles({
      name: 'SKILL.md',
      mimeType: 'text/markdown',
      buffer: Buffer.from(SKILL_MD)
    });

    await expect(component.getByText('Uploaded gah-manager@2.0.0.')).toBeVisible();
    expect(postedBody).toMatchObject({
      id: 'gah-manager',
      version: '2.0.0',
      displayName: 'GAH Manager',
      description: 'Orchestrates GAH work.',
      backends: ['hermes', 'codex'],
      content: SKILL_MD
    });
    await expect(component.locator('code', { hasText: 'gah-manager@2.0.0' })).toBeVisible();
    expect(listCalls).toBe(2);
  });

  test('defaults a missing version to 1.0.0', async ({ mount, page }) => {
    let postedBody: Record<string, unknown> | null = null;
    await page.route('**/api/skills', (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ skills: [] }) });
      }
      postedBody = route.request().postDataJSON();
      return route.fulfill({ status: 201, contentType: 'application/json', body: JSON.stringify({ ...postedBody, createdAt: 1, updatedAt: 1 }) });
    });

    const component = await mount(<SkillBankSettingsSection />);
    await component.locator('input[type="file"]').setInputFiles({
      name: 'SKILL.md',
      mimeType: 'text/markdown',
      buffer: Buffer.from('---\nid: no-version-skill\n---\n\nbody\n')
    });

    await expect(component.getByText('Uploaded no-version-skill@1.0.0.')).toBeVisible();
    expect(postedBody?.version).toBe('1.0.0');
  });

  test('shows a validation error and never calls the API when front matter has no id', async ({ mount, page }) => {
    let postCalled = false;
    await page.route('**/api/skills', (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ skills: [] }) });
      }
      postCalled = true;
      return route.fulfill({ status: 201, contentType: 'application/json', body: '{}' });
    });

    const component = await mount(<SkillBankSettingsSection />);
    await component.locator('input[type="file"]').setInputFiles({
      name: 'no-frontmatter.md',
      mimeType: 'text/markdown',
      buffer: Buffer.from('# Just a heading\n\nNo front matter here.\n')
    });

    await expect(component.getByRole('alert')).toContainText('no-frontmatter.md');
    await expect(component.getByRole('alert')).toContainText('id');
    expect(postCalled).toBe(false);
  });

  test('shows the API error message when the server rejects the upload', async ({ mount, page }) => {
    await page.route('**/api/skills', (route) => {
      if (route.request().method() === 'GET') {
        return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ skills: [] }) });
      }
      return route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Failed to store skill', message: 'boom: bad skill' })
      });
    });

    const component = await mount(<SkillBankSettingsSection />);
    await component.locator('input[type="file"]').setInputFiles({
      name: 'SKILL.md',
      mimeType: 'text/markdown',
      buffer: Buffer.from('---\nid: broken\n---\n\nbody\n')
    });

    await expect(component.getByRole('alert')).toContainText('boom: bad skill');
  });
});
