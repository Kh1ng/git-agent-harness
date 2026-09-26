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
    <CommitPrDialog profile="gah" sessionId="session-1" nodeId="worker-1" onClose={() => {}} onChanged={() => {}}
      mergeRequest={{ branch: 'feature/review', work_id: '#12', id: '12', url: 'https://github.com/owner/repo/pull/12', title: 'Existing work', state: 'OPEN', draft: false, merge_status: 'CLEAN', merged: false, ci_passed: true, ci_pending: false, review_contract_version: 1, classification: 'NEEDS_REVIEW', recommended_action: 'RUN_REVIEW' }} />
  );
  await expect(component.getByText('Windows worker')).toBeVisible();
  await expect(component.getByRole('region', { name: 'Provider review state' })).toContainText('NEEDS REVIEW');
  await expect(component.getByRole('region', { name: 'Provider review state' })).toContainText('RUN REVIEW');
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

  const publish = component.getByRole('button', { name: 'Push and create draft pull request' });
  await component.getByLabel('Base branch').fill('release');
  await expect(publish).toBeDisabled();
  await component.getByLabel('Base branch').fill('main');
  await expect(publish).toBeEnabled();
  page.once('dialog', dialog => dialog.accept());
  await publish.click();
  await expect(component.getByRole('link', { name: 'Open pull request' }).first()).toHaveAttribute('href', 'https://github.com/owner/repo/pull/12');
  expect(requests[1]).toEqual({
    path: '/api/git/publish',
    body: { title: 'Manual title', body: 'Manual body', base: 'main', draft: true, nodeId: 'worker-1' }
  });
});

test('read-only reviews expose state but disable editing and publishing', async ({ mount, page }) => {
  await page.route('**/api/git/review**', route => route.fulfill({ json: { ...initial, readOnly: true, existing: { number: 12, title: 'Existing work', url: 'https://github.com/owner/repo/pull/12', draft: false } } }));
  const component = await mount(<CommitPrDialog profile="gah" onClose={() => {}} onChanged={() => {}} />);
  await expect(component.getByLabel('Pull request title')).toHaveAttribute('readonly', '');
  await expect(component.getByLabel('Pull request body')).toHaveAttribute('readonly', '');
  await expect(component.getByText('This checkout is read-only.')).toBeVisible();
  await expect(component.getByRole('button', { name: /Push and update/ })).toBeDisabled();
  await expect(component.getByRole('button', { name: 'Update published title and description' })).toBeDisabled();
});

test('updates published prose without pushing and surfaces provider stderr', async ({ mount, page }) => {
  let update: Record<string, unknown> | null = null;
  let fail = false;
  await page.route('**/api/git/review**', route => route.fulfill({ json: {
    ...initial,
    files: [],
    existing: { number: 12, title: 'Existing work', url: 'https://github.com/owner/repo/pull/12', draft: false },
  } }));
  await page.route('**/api/git/pull-request/update**', async route => {
    update = route.request().postDataJSON();
    return fail
      ? route.fulfill({ status: 502, json: { error: 'Provider update failed', message: 'provider stderr detail' } })
      : route.fulfill({ json: { url: 'https://github.com/owner/repo/pull/12' } });
  });
  const component = await mount(<CommitPrDialog profile="gah" nodeId="worker-1" onClose={() => {}} onChanged={() => {}} />);
  await component.getByLabel('Pull request title').fill('Updated title');
  await component.getByLabel('Pull request body').fill('Updated description');
  page.once('dialog', dialog => dialog.accept());
  await component.getByRole('button', { name: 'Update published title and description' }).click();
  expect(update).toEqual({ number: 12, title: 'Updated title', body: 'Updated description', nodeId: 'worker-1' });
  await expect(component.getByRole('link', { name: 'Open pull request' }).first()).toHaveAttribute('href', 'https://github.com/owner/repo/pull/12');

  fail = true;
  await component.getByLabel('Pull request title').fill('Another title');
  page.once('dialog', dialog => dialog.accept());
  await component.getByRole('button', { name: 'Update published title and description' }).click();
  await expect(component.getByRole('alert')).toContainText('provider stderr detail');
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
        ? { kind: body.kind, text: 'Update selected files', generated: true, backend: 'codex', backendInstance: 'codex-work', requestedModel: null, effectiveModel: 'gpt-6-luna', actualModel: 'gpt-6-luna', fallbackReason: null, skippedFiles: ['.env'] }
        : { kind: body.kind, text: 'Summary', title: 'Improve review flow', body: '## Summary\n\n- Improve review', generated: true, backend: 'codex', backendInstance: 'codex-work', requestedModel: null, effectiveModel: 'gpt-6-luna', actualModel: 'gpt-6-luna', fallbackReason: null, skippedFiles: ['secrets.json'] } });
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
  await expect(component.getByText('Not sent to the helper: .env')).toBeVisible();
  await component.getByText('src/keep.ts').click();
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toHaveCount(0);
  await component.getByLabel('Commit message').fill('Manual commit');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toHaveCount(0);

  await component.getByRole('button', { name: 'Continue with uncommitted changes' }).click();
  await component.getByRole('button', { name: 'Suggest title and body' }).click();
  await expect(component.getByLabel('Pull request title')).toHaveValue('Improve review flow');
  await expect(component.getByLabel('Pull request body')).toHaveValue('## Summary\n\n- Improve review');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toBeVisible();
  await expect(component.getByText('Not sent to the helper: secrets.json')).toBeVisible();
  await component.getByLabel('Base branch').fill('develop');
  await expect(component.getByText('Suggested by codex-work · gpt-6-luna')).toHaveCount(0);
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
        backendInstance: null, requestedModel: null, effectiveModel: null, actualModel: null, fallbackReason: 'missing_model' } });
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
