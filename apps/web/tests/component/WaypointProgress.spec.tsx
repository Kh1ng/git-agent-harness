import { expect, test } from '@playwright/experimental-ct-react';
import type { LedgerEntry, MergeRequest } from '@git-agent-harness/contracts';
import { WaypointHistory } from '../../src/components/WaypointProgress.js';

const entries = [
  {
    timestamp: '2026-09-11T12:00:00Z',
    commit_created: true,
    validation_result: 'passed',
    mr_created: true
  },
  {
    timestamp: '2026-09-12T11:59:00Z',
    mode: 'clear_attempts'
  },
  {
    timestamp: '2026-09-12T12:00:00Z',
    dispatch_reason: 'initial',
    commit_created: true,
    validation_result: 'passed',
    mr_created: true
  }
] as LedgerEntry[];

const mergeRequest = {
  id: '42',
  merged: true,
  merged_at: '2026-09-12T12:30:00Z',
  ci_passed: true,
  merge_commit_sha: '1234567890abcdef'
} as MergeRequest;

test('derives and renders ticket history from existing durable records', async ({ mount }) => {
  const component = await mount(
    <WaypointHistory evidence={{ entries, mergeRequest }} label="#1077" />
  );

  await expect(component.getByRole('list', { name: '#1077 waypoint history' })).toBeVisible();
  await expect(component.locator('[aria-current="step"]')).toContainText('Merged');
  await expect(component.getByText('Merge commit 12345678')).toBeVisible();
  await expect(component.getByText('2026-09-11T12:00:00Z')).toHaveCount(0);
});

test('renders summary-only ledger progress in the project view', async ({ mount }) => {
  const component = await mount(
    <WaypointHistory
      evidence={{
        ledgerEvidence: {
          first_dispatch_at: '2026-09-12T12:00:00Z',
          first_commit_at: '2026-09-12T12:10:00Z',
          first_validation_at: '2026-09-12T12:20:00Z',
          first_pull_request_at: null
        }
      }}
      label="#1078"
    />
  );

  await expect(component.locator('[aria-current="step"]')).toContainText('Validated');
  await expect(component.getByText('Commit recorded in the ledger')).toBeVisible();
});
