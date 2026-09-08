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

for (const [label, classification, url, keyboard] of [
  ['View MR', 'NEEDS_REVIEW', 'https://github.com/example/project/pull/42', false],
  ['Merged change', 'MERGED', 'https://gitlab.example.com/group/project/-/merge_requests/43', true],
  ['View', 'MERGED', 'https://github.com/example/project/pull/44', false],
] as const) {
  test(`${label} navigates when the host disallows popups`, async ({ mount, page, context }) => {
    // Embedded hosts need not support opening a second browsing context.
    await page.route(page.url(), async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, headers: {
        ...response.headers(),
        'content-security-policy': 'sandbox allow-scripts allow-same-origin',
      } });
    });
    await page.reload();
    await context.route(url, (route) => route.fulfill({ contentType: 'text/html', body: '<h1>Provider merge request</h1>' }));
    const component = await mount(
      <MockStoreProvider statusData={{ ...snapshot, merge_requests: [{
        branch: 'change', id: '42', url, title: 'Merged change', classification,
        state: 'opened', draft: false, merge_status: null, merged: classification === 'MERGED',
        ci_passed: true, ci_pending: false, review_contract_version: 1, recommended_action: 'RUN_REVIEW',
      }] }}>
        <WebSocketProvider>
          <OverviewPage sessions={[]} onSelectSession={() => {}} onNavigate={() => {}} />
        </WebSocketProvider>
      </MockStoreProvider>,
    );
    const link = component.getByRole('link', { name: label, exact: true });
    await expect(link).toHaveAttribute('href', url);
    if (keyboard) {
      await link.focus();
      await page.keyboard.press('Enter');
    } else {
      await link.click();
    }
    await expect(page).toHaveURL(url);
    await expect(page.getByRole('heading', { name: 'Provider merge request' })).toBeVisible();
    expect(context.pages()).toHaveLength(1);
  });
}
