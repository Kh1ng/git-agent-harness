import { readFileSync } from 'node:fs';
import React from 'react';
import { test, expect } from '@playwright/experimental-ct-react';
import type { StatusSnapshot } from '@git-agent-harness/contracts';
import { OverviewPage } from '../../src/pages/OverviewPage.js';
import { MockStoreProvider } from '../../src/test-utils/MockStoreProvider.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';

const snapshot: StatusSnapshot = JSON.parse(readFileSync(
  new URL('../../../server/tests/fixtures/gah/responses/status.json', import.meta.url), 'utf8',
));

for (const classification of ['NEEDS_REVIEW', 'MERGED']) {
  test(`${classification} rows open work details without provider navigation`, async ({ mount, page }) => {
    let selected = '';
    const component = await mount(
      <MockStoreProvider statusData={{ ...snapshot, merge_requests: [{
        branch: 'change', work_id: '#42', id: '42', url: 'https://github.com/example/project/pull/42', title: 'Review change', classification,
        state: 'opened', draft: false, merge_status: null, merged: classification === 'MERGED',
        ci_passed: true, ci_pending: false, review_contract_version: 1, recommended_action: 'RUN_REVIEW',
      }] }}>
        <WebSocketProvider>
          <OverviewPage
            sessions={[]}
            onSelectSession={() => {}}
            onNavigate={() => {}}
            onOpenWork={(workId) => { selected = workId; }}
          />
        </WebSocketProvider>
      </MockStoreProvider>,
    );
    const before = page.url();
    await component.getByRole('button', { name: 'View details', exact: true }).click();
    await expect.poll(() => selected).toBe('#42');
    await expect(page).toHaveURL(before);
  });
}
