import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { GitReviewState } from '@git-agent-harness/contracts';
import { CommitPrDialog } from '../../src/components/CommitPrDialog.js';

const initial: GitReviewState = {
  ownerNodeId: 'worker-1',
  ownerNodeName: 'Windows worker',
  provider: 'github',
  providerLabel: 'pull request',
  branch: 'feature/review',
  base: 'main',
  upstream: null,
  ahead: 1,
  behind: 0,
  files: [
    { path: 'src/alpha.ts', staged: false, unstaged: true, untracked: false },
    { path: 'src/keep.ts', staged: true, unstaged: false, untracked: false }
  ],
  commits: [{ hash: '1111111', short: '1111111', subject: 'Existing work' }],
  changedFiles: ['README.md'],
  patch: 'diff --git a/README.md b/README.md',
  existing: null
};

test('commits selected files, preserves excluded work, and publishes only after final review', async ({ mount, page }) => {
  const requests: Array<{ path: string; body: Record<string, unknown> }> = [];
  let reviewReads = 0;
  await page.route('**/api/git/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === '/api/git/review') {
      reviewReads += 1;
      const review = reviewReads === 1 ? initial : {
        ...initial,
        files: [initial.files[1]],
        commits: [{ hash: '2222222', short: '2222222', subject: 'Update alpha' }, ...initial.commits],
        changedFiles: ['README.md', 'src/alpha.ts'],
        patch: 'diff --git a/src/alpha.ts b/src/alpha.ts'
      };
      return route.fulfill({ json: review });
    }
    requests.push({ path: url.pathname, body: request.postDataJSON() });
    if (url.pathname === '/api/git/commit') return route.fulfill({ json: { hash: '2222222' } });
    return route.fulfill({ json: { url: 'https://github.com/owner/repo/pull/12', existing: false } });
  });

  const component = await mount(
    <CommitPrDialog profile="gah" sessionId="session-1" nodeId="worker-1" onClose={() => {}} onChanged={() => {}} />
  );
  await expect(component.getByText('Windows worker')).toBeVisible();
  await component.getByText('src/keep.ts').click();
  await component.getByLabel('Commit message').fill('Update alpha');
  await component.getByRole('button', { name: 'Commit selected' }).click();
  await expect(component.getByText('1 of 1 selected')).toBeVisible();

  expect(requests[0]).toEqual({
    path: '/api/git/commit',
    body: { message: 'Update alpha', files: ['src/alpha.ts'], nodeId: 'worker-1' }
  });
  await component.getByRole('button', { name: 'Continue with uncommitted changes' }).click();
  await expect(component.getByText('src/keep.ts')).toHaveCount(2);
  await component.getByLabel('Pull request title').fill('Manual title');
  await component.getByLabel('Pull request body').fill('Manual body');
  await component.getByLabel('Draft').check();
  expect(requests).toHaveLength(1);

  await component.getByRole('button', { name: 'Push and create draft pull request' }).click();
  await expect(component.getByRole('link', { name: 'Open pull request' })).toHaveAttribute('href', 'https://github.com/owner/repo/pull/12');
  expect(requests[1]).toEqual({
    path: '/api/git/publish',
    body: { title: 'Manual title', body: 'Manual body', base: 'main', draft: true, nodeId: 'worker-1' }
  });
});

test('requested model suggestions stay attributed until the user edits them', async ({ mount, page }) => {
  const suggestions: Record<string, unknown>[] = [];
  await page.route('**/api/git/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === '/api/git/review') return route.fulfill({ json: initial });
    if (url.pathname === '/api/git/suggest') {
      const body = request.postDataJSON();
      suggestions.push(body);
      return route.fulfill({ json: body.kind === 'commit_message'
        ? { kind: body.kind, text: 'Update selected files', generated: true, backend: 'codex', backendInstance: 'codex-work', model: 'gpt-6-luna', fallbackReason: null }
        : { kind: body.kind, text: 'Summary', title: 'Improve review flow', body: '## Summary\n\n- Improve review', generated: true, backend: 'codex', backendInstance: 'codex-work', model: 'gpt-6-luna', fallbackReason: null } });
    }
    return route.abort();
  });

  const component = await mount(
    <CommitPrDialog profile="gah" sessionId="session-1" nodeId="worker-1" onClose={() => {}} onChanged={() => {}} />
  );
  await component.getByText('src/keep.ts').click();
  await component.getByRole('button', { name: 'Suggest' }).click();
  await expect(component.getByLabel('Commit message')).toHaveValue('Update selected files');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toBeVisible();
  await component.getByLabel('Commit message').fill('Manual commit');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toHaveCount(0);

  await component.getByRole('button', { name: 'Continue with uncommitted changes' }).click();
  await component.getByRole('button', { name: 'Suggest title and body' }).click();
  await expect(component.getByLabel('Pull request title')).toHaveValue('Improve review flow');
  await expect(component.getByLabel('Pull request body')).toHaveValue('## Summary\n\n- Improve review');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toBeVisible();
  await component.getByLabel('Pull request title').fill('Manual PR');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toHaveCount(0);

  expect(suggestions).toEqual([
    { kind: 'commit_message', sessionId: 'session-1', nodeId: 'worker-1', files: ['src/alpha.ts'] },
    { kind: 'pr_summary', sessionId: 'session-1', nodeId: 'worker-1', base: 'main' }
  ]);
});

test('late and failed suggestions preserve user prose', async ({ mount, page }) => {
  await page.route('**/api/git/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/git/review') return route.fulfill({ json: initial });
    if (url.pathname === '/api/git/suggest') {
      await new Promise(resolve => setTimeout(resolve, 100));
      return route.fulfill({ json: { kind: 'commit_message', text: '', generated: false, backend: null,
        backendInstance: null, model: null, fallbackReason: 'missing_model' } });
    }
    return route.abort();
  });
  const component = await mount(<CommitPrDialog profile="gah" onClose={() => {}} onChanged={() => {}} />);
  await component.getByText('src/keep.ts').click();
  await component.getByRole('button', { name: 'Suggest' }).click();
  await component.getByLabel('Commit message').fill('Keep my message');
  await expect(component.getByRole('button', { name: 'Suggest' })).toBeEnabled();
  await expect(component.getByLabel('Commit message')).toHaveValue('Keep my message');

  page.once('dialog', dialog => dialog.accept());
  await component.getByRole('button', { name: 'Suggest' }).click();
  await expect(component.getByRole('alert')).toContainText('missing_model');
  await expect(component.getByLabel('Commit message')).toHaveValue('Keep my message');
});
