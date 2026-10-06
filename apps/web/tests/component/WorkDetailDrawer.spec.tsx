import { readFileSync } from 'node:fs';
import React from 'react';
import { expect, test } from '@playwright/experimental-ct-react';
import type { LedgerEntry, StatusSnapshot } from '@git-agent-harness/contracts';
import { WorkDetailDrawer } from '../../src/components/WorkDetailDrawer.js';
import { MockStoreProvider } from '../../src/test-utils/MockStoreProvider.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

const baseStatus: StatusSnapshot = JSON.parse(readFileSync(
  new URL('../../../server/tests/fixtures/gah/responses/status.json', import.meta.url), 'utf8',
));

const entry = {
  timestamp: '2026-09-26T12:00:00Z', profile: 'fixture', display_name: 'Drawer work', repo_id: 'repo', repo: 'Kh1ng/git-agent-harness', local_path: '/tmp/repo', provider: 'github',
  backend: 'codex', requested_backend: 'codex', effective_backend: 'codex', requested_model: 'gpt-5', effective_model: 'gpt-5', routing_reason: null, fallback_used: false,
  confidence_impact: null, human_required: false, mode: 'fix', target_summary: 'Drawer work', work_id: '#42', work_title: 'Drawer work', branch: 'feat/42', session_id: 'session-42',
  session_dir: '/tmp/session', duration_seconds: 12, backend_exit_code: 0, validation_result: 'passed', commit_attempted: true, commit_created: true, push_attempted: true,
  push_succeeded: true, mr_attempted: true, mr_created: true, mr_url: 'https://github.com/example/repo/pull/42', files_changed: 1, insertions: 2, deletions: 0,
  error_summary: null, dispatch_reason: 'initial', attempts: [], usage: { usage_source: 'backend_reported', input_tokens: 10, output_tokens: 5, cache_read_tokens: 0, cache_write_tokens: 0,
    total_tokens: 15, requests_count: 1, estimated_cost_usd: 0.01, actual_cost_usd: null, quota_window: null, quota_remaining_percent: null,
    quota_reset_at: null, provider: 'openai', actual_model: 'gpt-5', actual_model_unknown_reason: null, provider_unknown_reason: null, account_label: null, auth_source_label: null,
    quota_pool: null, pricing_source: null, pricing_version: null, cost_unknown_reason: null, observed_at: null },
} as LedgerEntry;

function status(held = false): StatusSnapshot {
  return {
    ...baseStatus,
    review_held_work_ids: held ? ['#42'] : [],
    available_tickets: [{
      ticket_path: '42', normalized_work_identity: '#42', work_id: '#42', title: 'Drawer work', source: 'provider_issue', execution_policy: { dispatchable_now: true, reasons: [] },
      recommended_backend: 'codex', recommended_model: 'gpt-5', prior_attempt_count: 1, genuine_agent_failure_count: 0, last_failure_class: null,
      has_active_mr: true, has_active_claim: false, human_required: false,
    }],
    merge_requests: [{
      branch: 'feat/42', work_id: '#42', id: '42', url: 'https://github.com/example/repo/pull/42', title: 'Drawer work', state: 'open', draft: false,
      merge_status: 'mergeable', merged: false, ci_passed: true, ci_pending: false, review_contract_version: 1, classification: 'NEEDS_REVIEW', recommended_action: 'RUN_REVIEW',
    }],
  };
}

async function routes(page: import('@playwright/test').Page) {
  await page.route('**/api/work/**', (route) => route.fulfill({ json: [entry] }));
  await page.route('**/api/profiles', (route) => route.fulfill({ json: [{ name: 'fixture', repo: 'Kh1ng/git-agent-harness' }] }));
  await page.route('**/api/hold/set', (route) => route.fulfill({ json: { success: true } }));
  await page.route('**/api/hold/clear', (route) => route.fulfill({ json: { success: true } }));
  await page.route('**/api/ledger/clear-attempts', (route) => route.fulfill({ json: { success: true } }));
  await page.route('**/api/git/review**', (route) => route.fulfill({ json: {
    ownerNodeId: 'local', ownerNodeName: 'Local node', provider: 'github', providerLabel: 'pull request', branch: 'feat/42', base: 'main', upstream: 'origin/feat/42',
    ahead: 0, behind: 0, files: [], commits: [{ hash: 'abcdef1', short: 'abcdef1', subject: 'Drawer work' }], changedFiles: ['README.md'], patch: '',
    existing: { number: 42, title: 'Drawer work', url: 'https://github.com/example/repo/pull/42', draft: false },
  } }));
}

test('renders review and attempt evidence and runs hold, clear-attempts, and redispatch controls', async ({ mount, page }) => {
  await routes(page);
  page.on('dialog', (dialog) => dialog.accept());
  const component = await mount(
    <MockStoreProvider statusData={status()}><WebSocketProvider>
      <WorkDetailDrawer workId="#42" profile="fixture" connected sessions={[]} onClose={() => {}} onRedispatch={() => {}} />
    </WebSocketProvider></MockStoreProvider>,
  );
  await expect(component.getByRole('heading', { name: 'Drawer work' })).toBeVisible();
  await expect(component.getByText('Needs review', { exact: true })).toBeVisible();
  await expect(component.getByText('RUN REVIEW')).toBeVisible();
  await expect(component.getByText('codex/gpt-5', { exact: true }).first()).toBeVisible();
  await expect(component.getByText('$0.0100', { exact: true })).toBeVisible();

  await component.getByRole('button', { name: 'Set hold' }).click();
  await expect(component.getByRole('status')).toHaveText('Hold set.');
  await component.getByRole('button', { name: 'Clear attempts' }).click();
  await expect(component.getByRole('status')).toHaveText('Prior attempts cleared.');
  await component.getByRole('button', { name: 'Re-dispatch' }).click();
  await expect(component.getByRole('status')).toHaveText('Dispatch queued. The activity feed reports the outcome.');
  await component.getByRole('button', { name: 'Review in dashboard' }).click();
  await expect(page.getByRole('dialog', { name: 'Commit and pull request review' })).toBeVisible();
});

test('clears an active hold', async ({ mount, page }) => {
  await routes(page);
  const component = await mount(
    <MockStoreProvider statusData={status(true)}><WebSocketProvider>
      <WorkDetailDrawer workId="#42" profile="fixture" connected sessions={[]} onClose={() => {}} onRedispatch={() => {}} />
    </WebSocketProvider></MockStoreProvider>,
  );
  await component.getByRole('button', { name: 'Clear hold' }).click();
  await expect(component.getByRole('status')).toHaveText('Hold cleared.');
});
