import { expect, test } from '@playwright/experimental-ct-react';
import type { ControllerActivity } from '@git-agent-harness/contracts';
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

test('keeps terminal controller history collapsed when no work is running', async ({ mount }) => {
  const component = await mount(<ControllerActivityCard activity={terminalRuns} />);

  await expect(component.getByText('Idle', { exact: true })).toBeVisible();
  await expect(component.getByText('Recent history', { exact: true })).toBeVisible();
  await expect(component.getByText(/1 failed.*1 finished/)).toBeVisible();
  await expect(component.getByText('unassigned', { exact: true })).toHaveCount(0);
  await expect(component.getByText(terminalRuns[0].action, { exact: true })).toBeHidden();

  await component.getByText('Recent history', { exact: true }).click();
  await expect(component.getByText(terminalRuns[0].action, { exact: true })).toBeVisible();
});
