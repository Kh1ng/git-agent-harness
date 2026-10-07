import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { RunningWorker } from '@git-agent-harness/contracts';
import { RunningWorkersRoster } from '../../src/components/RunningWorkersRoster.js';
const worker: RunningWorker = { work_id: '#1431', run_id: 'run', backend: 'codex-work', runner: 'codex', backend_instance: 'work-account', mode: 'review', model: 'routed', requested_model: 'requested', actual_model: null, node_id: 'node-a', branch: 'gah/1431', started_at: '2026-10-06T00:00:00Z', last_activity_at: '2026-10-06T00:05:00Z', attempt: 2, stale_after_seconds: 900, state: 'running' };
test('empty, populated and stale roster preserve routing facts and narrow layout', async ({ mount, page }) => {
  await page.setViewportSize({ width: 320, height: 800 });
  const component = await mount(<RunningWorkersRoster workers={[]} />);
  await expect(component).toContainText('Running workers (0)');
  await expect(component).toContainText('No running workers reported.');
  await component.update(<RunningWorkersRoster workers={[worker, { ...worker, run_id: 'stale', state: 'stale', model: null, node_id: 'remote-' + 'x'.repeat(100) }]} />);
  await expect(component.getByTestId('worker-row')).toHaveCount(2);
  await expect(component).toContainText('Running workers (2)');
  await expect(component).toContainText('Runner: codex · Backend: codex-work · Model: routed');
  await expect(component).toContainText('attempt 2');
  await expect(component).toContainText('stale');
  await expect(component).toContainText('Model: Unknown');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
