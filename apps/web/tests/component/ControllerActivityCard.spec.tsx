import React from 'react';
import { MockStoreProvider } from '../../src/test-utils/MockStoreProvider.js';
import { RunningWorkersRoster } from '../../src/components/RunningWorkersRoster.js';
import { expect, test } from '@playwright/experimental-ct-react';
import type { ControllerActivity, StatusSnapshot, RunningWorker } from '@git-agent-harness/contracts';
import { ControllerActivityCard } from '../../src/components/ControllerActivityCard.js';

const terminalRuns: ControllerActivity[] = [
  {
    run_id: '11111111-1111-1111-1111-111111111111',
    profile: 'gah',
    work_id: null,
    started_at: '2026-09-04T20:00:00Z',
    finished_at: '2026-09-04T20:05:00Z',
    action: 'dispatch: Review PR #1117 with a very long prompt that should not dominate the page',
    status: 'failed',
    outcome: 'dispatch: git fetch failed'
  },
  {
    run_id: '22222222-2222-2222-2222-222222222222',
    profile: 'gah',
    work_id: '#1113',
    started_at: '2026-09-04T19:00:00Z',
    finished_at: '2026-09-04T19:30:00Z',
    action: 'dispatch: Repair PR #1113',
    status: 'finished',
    outcome: 'dispatch: success'
  }
];

const running: ControllerActivity = {
  run_id: '33333333-3333-3333-3333-333333333333',
  profile: 'gah',
  work_id: '#1112',
  started_at: '2026-09-04T21:00:00Z',
  finished_at: null,
  action: 'dispatch: Improve #1112',
  status: 'running',
  outcome: null
};

test('keeps terminal controller history collapsed when no work is running', async ({ mount }) => {
  const component = await mount(<ControllerActivityCard activity={terminalRuns} />);

  await expect(component.getByText('Idle', { exact: true })).toBeVisible();
  await expect(component.getByText('Recent history', { exact: true })).toBeVisible();
  await expect(component.getByText(/1 failed.*1 finished/)).toBeVisible();
  await expect(component.getByText('unassigned', { exact: true })).toHaveCount(0);
  await expect(component.getByText('Review PR #1117 with a very long prompt that should not dominate the page', { exact: true })).toBeHidden();

  await component.getByText('Recent history', { exact: true }).click();
  await expect(component.getByText('Review PR #1117 with a very long prompt that should not dominate the page', { exact: true })).toBeVisible();
  await component.getByText('Review PR #1117 with a very long prompt that should not dominate the page', { exact: true }).click();
  await expect(component.getByText(terminalRuns[0].action, { exact: true })).toBeVisible();
});

test('running count comes from the roster even when controller events disagree', async ({ mount }) => {
  const worker: RunningWorker = { work_id: '#1112', run_id: 'run', mode: 'improve', backend: 'codex', runner: 'codex', backend_instance: 'codex-work', requested_model: 'requested', model: 'routed', actual_model: null, node_id: 'node', branch: 'gah/1112', started_at: running.started_at, last_activity_at: running.started_at, attempt: 2, stale_after_seconds: 900, state: 'stale' };
  const workers = [worker, { ...worker, run_id: 'second', state: 'running' as const }];
  const status = { running_workers: workers } as StatusSnapshot;
  const component = await mount(<MockStoreProvider statusData={status}><ControllerActivityCard activity={[running]} /><RunningWorkersRoster workers={workers} /></MockStoreProvider>);
  await expect(component.getByText('2 running', { exact: true })).toBeVisible();
  await expect(component.getByTestId('worker-row')).toHaveCount(2);
  await expect(component.getByText('View worker roster')).toHaveAttribute('href', '#running-workers');
  await component.update(<MockStoreProvider statusData={{ running_workers: [] } as unknown as StatusSnapshot}><ControllerActivityCard activity={[running]} /><RunningWorkersRoster workers={[]} /></MockStoreProvider>);
  await expect(component.getByText('Idle', { exact: true })).toBeVisible();
  await expect(component.getByTestId('worker-row')).toHaveCount(0);
});
