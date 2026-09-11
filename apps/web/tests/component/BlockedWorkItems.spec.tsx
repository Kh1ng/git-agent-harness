import { test, expect } from '@playwright/experimental-ct-react';
import type { Blocker, RemediationPlan } from '@git-agent-harness/contracts';
import { BlockedWorkItems } from '../../src/components/BlockedWorkItems.js';
import React from 'react';

const planBlocker: Blocker = {
  kind: 'human_required',
  message: 'needs external API approval',
  backend: 'codex',
  model: 'gpt-5.4-mini',
  until: '2026-09-11T03:00:00Z',
  source_reference: '#653',
  reason_code: 'external_api_approval_required',
  remediation_plan: {
    result: 'plan',
    profile: 'test',
    work_id: '#653',
    reason_code: 'external_api_approval_required',
    required_authority: 'operator',
    safe_actions: [
      {
        kind: 'command',
        summary: 'Grant the exact external-approval request',
        command: 'gah external-approval grant --profile test "#653" --credential-label odds --operation-kind env_credential',
      },
      {
        kind: 'inspect',
        summary: 'Inspect the pending request',
        api_action: 'Check the ledger for the pending request bounds',
      },
    ],
  },
};

const legacyBlocker: Blocker = {
  kind: 'human_required',
  message: 'some legacy hold',
  source_reference: '#1',
  reason_code: 'unknown',
};

test('renders typed reason code, attempted route, and copyable remediation commands', async ({ mount }) => {
  const component = await mount(<BlockedWorkItems blockers={[planBlocker]} />);
  await expect(component).toContainText('#653');
  await expect(component).toContainText('external_api_approval_required');
  await expect(component).toContainText('codex/gpt-5.4-mini');
  await expect(component).toContainText('next eligible: 2026-09-11T03:00:00Z');
  await expect(component).toContainText('operator');
  await expect(component).toContainText('gah external-approval grant');
  // no_automatic_remediation text is absent for a plan result
  await expect(component).not.toContainText('No remediation plan');
});

test('unknown and legacy reasons stay visibly unknown with no plan', async ({ mount }) => {
  const component = await mount(<BlockedWorkItems blockers={[legacyBlocker]} />);
  await expect(component).toContainText('Unknown reason');
  await expect(component).toContainText('No remediation plan for this reason code.');
});

test('budget-capped reasons surface the clear-attempts control', async ({ mount }) => {
  const capped: Blocker = {
    ...planBlocker,
    source_reference: '#42',
    reason_code: 'retry_budget_exhausted',
    remediation_plan: {
      result: 'plan',
      profile: 'test',
      work_id: '#42',
      reason_code: 'retry_budget_exhausted',
      required_authority: 'operator',
      safe_actions: [
        { kind: 'command', summary: 'Clear attempts', command: 'gah ledger clear-attempts --profile test "#42"' },
      ],
    } satisfies RemediationPlan,
  };
  const component = await mount(<BlockedWorkItems blockers={[capped]} />);
  await expect(component).toContainText('Clear attempts & retry');
});
