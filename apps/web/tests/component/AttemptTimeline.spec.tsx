import { expect, test } from '@playwright/experimental-ct-react';
import type {
  LedgerEntry,
  LedgerUsage,
  RoutingDiagnostics
} from '@git-agent-harness/contracts';
import { AttemptTimeline } from '../../src/components/AttemptTimeline.js';

const reviewerDiagnostics: RoutingDiagnostics = {
  policy_reordered_candidates: true,
  selected_backend: 'reviewer-backend',
  selected_model: 'gpt-4.1',
  selected_quota_pool: 'reviewers',
  selected_pace_band: 'normal',
  selected_cost_class: 'standard',
  selected_over: ['fallback-reviewer/gpt-4o'],
  human_summary: 'Selected the quota-backed reviewer after skipping an exhausted alternative.',
  candidates: [
    {
      backend: 'reviewer-backend',
      model: 'gpt-4.1',
      quota_pool: 'reviewers',
      default_order: 1,
      consideration_order: 1,
      pace_band: 'normal',
      cost_class: 'standard',
      skip_reason: null,
      unavailable_until: null
    },
    {
      backend: 'fallback-reviewer',
      model: 'gpt-4o',
      quota_pool: 'fallback',
      default_order: 2,
      consideration_order: 2,
      pace_band: 'fast',
      cost_class: 'premium',
      skip_reason: 'quota exhausted',
      unavailable_until: '2026-07-22T15:00:00Z'
    }
  ]
};

const baseUsage: LedgerUsage = {
  usage_source: 'backend_reported',
  usage_classification: 'quota_backed',
  backend_instance: 'reviewer-backend-0',
  provider: 'openai',
  actual_model: 'gpt-4.1',
  actual_model_unknown_reason: null,
  provider_unknown_reason: null,
  account_label: 'team-a',
  auth_source_label: 'shared-quota',
  quota_pool: 'reviewers',
  provider_attribution_source: 'backend_reported',
  pricing_source: 'ledger',
  pricing_version: '2026.07',
  cost_unknown_reason: null,
  observed_at: '2026-07-22T14:25:00Z',
  input_tokens: 1200,
  output_tokens: 350,
  reasoning_tokens: 80,
  cache_read_tokens: 10,
  cache_write_tokens: 0,
  total_tokens: 1640,
  requests_count: 3,
  estimated_cost_usd: 1.23,
  actual_cost_usd: 1.1,
  quota_window: '2026-07-22T14:00:00Z/2026-07-22T15:00:00Z',
  quota_used_percent: 0.62,
  quota_remaining_percent: 0.38,
  quota_reset_at: '2026-07-22T15:00:00Z',
  token_usage_unknown_reason: null,
  quota_unknown_reason: null,
  behavior_metrics: {
    tool_calls: { count: 4, quality: 'provider_reported' },
    shell_calls: { count: 2, quality: 'provider_reported' },
    file_edits: { count: 1, quality: 'provider_reported' },
    test_runs: { count: 1, quality: 'provider_reported' }
  }
};

const entry: LedgerEntry = {
  timestamp: '2026-07-22T14:30:00Z',
  session_id: 'sess-1',
  profile: 'gah',
  display_name: 'Review work item',
  repo_id: 'repo-1',
  repo: 'Kh1ng/git-agent-harness',
  local_path: '/home/khing/workspace/agent-lab/worktrees/gah-gah-1784753931-d401d4fbfa624411832d726f3e717a37',
  provider: 'github',
  backend: 'reviewer-backend',
  requested_backend: 'auto',
  effective_backend: 'reviewer-backend',
  requested_model: 'gpt-4.1',
  effective_model: 'gpt-4.1',
  routing_reason: 'Selected the quota-backed reviewer after skipping an exhausted alternative.',
  fallback_used: false,
  confidence_impact: 'raised confidence because the selected reviewer had available quota.',
  human_required: false,
  routing_diagnostics: reviewerDiagnostics,
  mode: 'review',
  target_summary: 'Review routing and reviewer attribution rendering',
  branch: 'gah-751-review',
  session_dir: '/tmp/gah/session-1',
  duration_seconds: 95,
  backend_exit_code: 0,
  validation_result: 'passed',
  review_verdict: 'APPROVE_WEAK',
  review_confidence: '0.82',
  reviewer_backend: 'reviewer-backend',
  reviewer_model: 'gpt-4.1',
  review_gate_reason: null,
  review_contract_version: 1,
  review_generation: 'final',
  review_timeout_class: 'completed',
  review_idle_timeout_seconds: 180,
  review_hard_timeout_seconds: 900,
  review_last_progress_secs: 32,
  commit_attempted: true,
  commit_created: true,
  push_attempted: true,
  push_succeeded: false,
  mr_attempted: true,
  mr_created: false,
  mr_url: 'https://example.com/mr/123',
  files_changed: 5,
  insertions: 12,
  deletions: 4,
  error_summary: null,
  failure_class: null,
  failure_stage: null,
  attempts_started: 1,
  attempts_completed: 1,
  attempts: [
    {
      attempt_number: 1,
      backend: 'reviewer-backend',
      effective_model: 'gpt-4.1',
      exit_code: 0,
      validation_result: 'passed',
      failure_class: null,
      failure_stage: null,
      duration_seconds: 95,
      diff_path: '/tmp/diff.patch',
      usage: {
        ...baseUsage,
        backend_instance: 'reviewer-backend-0',
        estimated_cost_usd: 1.23,
        actual_cost_usd: 1.1
      }
    }
  ],
  attempt_routing: [
    {
      attempt_number: 1,
      backend_instance: 'reviewer-backend-0',
      effective_model: 'gpt-4.1',
      routing_diagnostics: reviewerDiagnostics
    }
  ],
  dispatch_reason: 'review',
  usage: {
    ...baseUsage,
    backend_instance: 'reviewer-backend-0'
  }
};

test('renders reviewer attribution, routing rationale, diff stats, and outcome booleans', async ({ mount }) => {
  const component = await mount(<AttemptTimeline entries={[entry]} />);

  await expect(component).toContainText('Reviewed by reviewer-backend/gpt-4.1');
  await expect(component).toContainText('Routing rationale');
  await expect(component).toContainText('Reason: Selected the quota-backed reviewer after skipping an exhausted alternative.');
  await expect(component).toContainText('Selected candidate: Selected reviewer-backend/gpt-4.1 · quota reviewers · pace normal · cost standard');
  await expect(component).toContainText('skipped: quota exhausted');
  await expect(component).toContainText('Files changed: 5');
  await expect(component).toContainText('+12 / -4');
  await expect(component).toContainText('Commit: created');
  await expect(component).toContainText('Push: not succeeded');
  await expect(component).toContainText('MR: not created');
  await expect(component).toContainText('Attempt routing');
  await expect(component).toContainText('Selected candidate: Selected reviewer-backend/gpt-4.1 · quota reviewers · pace normal · cost standard');
});
