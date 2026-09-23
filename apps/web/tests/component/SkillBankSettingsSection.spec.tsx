import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';
import { test, expect } from '@playwright/experimental-ct-react';
import { SkillBankSettingsSection } from '../../src/components/SkillBankSettingsSection.js';
import React from 'react';

const SKILL_MD = `---
id: gah-manager
version: 2.0.0
displayName: GAH Manager
description: |
  Orchestrates GAH work.
  Keeps dispatches focused.
backends: [hermes, codex]
---

# Role: GAH Manager
`;

test.beforeEach(async ({ page }) => { await page.routeWebSocket('**/ws*', socket => socket.close()); });

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

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
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

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
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
      description: 'Orchestrates GAH work.\nKeeps dispatches focused.',
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

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
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

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
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

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
    await component.locator('input[type="file"]').setInputFiles({
      name: 'SKILL.md',
      mimeType: 'text/markdown',
      buffer: Buffer.from('---\nid: broken\n---\n\nbody\n')
    });

    await expect(component.getByRole('alert')).toContainText('boom: bad skill');
  });

  test('keeps an edited draft after an owner denial and saves the same id and version on retry', async ({ mount, page }, testInfo) => {
    const skill = {
      id: 'gah-manager',
      version: '1.0.0',
      displayName: 'GAH Manager',
      description: 'Original description',
      backends: ['codex'],
      source: 'docs/gah-manager-skill.md',
      content: '# Original content',
      createdAt: 10,
      updatedAt: 10
    };
    let saveAttempts = 0;
    let savedBody: Record<string, unknown> | null = null;
    await page.route('**/api/skills/gah-manager?version=1.0.0', route => route.fulfill({ json: skill }));
    await page.route('**/api/skills', route => {
      if (route.request().method() === 'GET') {
        return route.fulfill({ json: { skills: [{ ...skill, bound: false }] } });
      }
      saveAttempts += 1;
      savedBody = route.request().postDataJSON();
      if (saveAttempts === 1) {
        return route.fulfill({ status: 403, json: { error: 'Forbidden', message: 'This operation requires owner access.' } });
      }
      return route.fulfill({ status: 200, json: { ...skill, ...savedBody, updatedAt: 20 } });
    });

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
    await component.getByRole('button', { name: 'Edit GAH Manager 1.0.0' }).click();
    await component.getByLabel('Display name').fill('Manager instructions');
    await component.getByLabel('Description').fill('Edited description');
    await component.getByLabel('Backend compatibility').fill('claude, codex');
    await component.getByLabel('Markdown content').fill('# Edited content');
    await component.getByRole('button', { name: 'Save changes' }).click();

    await expect(component.getByRole('alert')).toHaveText('This operation requires owner access.');
    await expect(component.getByLabel('Display name')).toHaveValue('Manager instructions');
    await expect(component.getByLabel('Markdown content')).toHaveValue('# Edited content');

    await component.getByRole('button', { name: 'Save changes' }).click();
    await expect(component.getByRole('status')).toContainText('Saved gah-manager@1.0.0.');
    expect(savedBody).toMatchObject({
      id: 'gah-manager',
      version: '1.0.0',
      displayName: 'Manager instructions',
      description: 'Edited description',
      backends: ['claude', 'codex'],
      content: '# Edited content',
      source: 'docs/gah-manager-skill.md'
    });
    await page.screenshot({ path: testInfo.outputPath('skill-editor-desktop.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: testInfo.outputPath('skill-editor-mobile.png') });
  });

  test('confirms removal of an unbound skill and explains why a bound skill cannot be removed', async ({ mount, page }) => {
    const summaries = [
      { id: 'bound', version: '1.0.0', displayName: 'Bound skill', description: '', backends: [], source: 'api', bound: true },
      { id: 'unused', version: '2.0.0', displayName: 'Unused skill', description: '', backends: [], source: 'api', bound: false }
    ];
    let removed = false;
    await page.route('**/api/skills/*', route => {
      const url = new URL(route.request().url());
      const id = url.pathname.split('/').at(-1)!;
      if (route.request().method() === 'DELETE') {
        removed = true;
        return route.fulfill({ json: { removed: 1 } });
      }
      const summary = summaries.find(skill => skill.id === id)!;
      return route.fulfill({ json: { ...summary, content: `# ${summary.displayName}`, createdAt: 1, updatedAt: 1 } });
    });
    await page.route('**/api/skills', route => route.fulfill({
      json: { skills: removed ? summaries.filter(skill => skill.id !== 'unused') : summaries }
    }));

    const component = await mount(<WebSocketProvider><SkillBankSettingsSection /></WebSocketProvider>);
    await component.getByRole('button', { name: 'Edit Bound skill 1.0.0' }).click();
    await expect(component.getByText('Removal is unavailable while this skill is bound. Unbind it from every project and backend first.')).toBeVisible();
    await expect(component.getByRole('button', { name: 'Remove skill' })).toBeDisabled();
    await component.getByRole('button', { name: 'Cancel' }).click();

    await component.getByRole('button', { name: 'Edit Unused skill 2.0.0' }).click();
    page.once('dialog', async dialog => {
      expect(dialog.message()).toBe("Remove 'unused' and all installed versions? This cannot be undone.");
      await dialog.accept();
    });
    await component.getByRole('button', { name: 'Remove skill' }).click();
    await expect(component.getByRole('status')).toContainText("Removed 'unused' and all installed versions.");
    await expect(component.getByText('unused@2.0.0')).toHaveCount(0);
  });
});
