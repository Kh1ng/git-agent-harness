import { test, expect } from '@playwright/experimental-ct-react';
import type { SettingsConfigProfileSummary } from '@git-agent-harness/contracts';
import { ProfileConfigViewerSection } from '../../src/pages/SettingsPage.js';
import React from 'react';

test('environment sources render configured status without rendering their values', async ({ mount }) => {
  const canary = 'TDAI_GATEWAY_API_KEY=GAH_TEST_CANARY_1014_DO_NOT_RENDER';
  const config: SettingsConfigProfileSummary & {
    notifications: SettingsConfigProfileSummary['notifications'] & {
      env_file: string;
      env_file_prod: string;
    };
  } = {
    profile: 'test-repo',
    delivery_mode: 'pr',
    merge_policy: 'auto',
    max_fix_attempts_per_mr: 2,
    max_implementation_failures_per_ticket: 8,
    max_review_cycles_per_ticket: 3,
    max_paid_reviews_per_ticket: 3,
    backend_instances: [],
    pm_candidates: [],
    improve_candidates: [],
    review_candidates: [],
    task_routing_rules: [],
    routine_reviewer: null,
    escalatory_reviewers: [],
    context: {
      global: {
        enabled: true,
        soft_limit_tokens: 80_000,
        hard_limit_tokens: 150_000,
        compact_after_tool_calls: 20,
        fresh_context_on_review: true,
        fresh_context_on_fix: true,
        include_full_git_history: false,
        include_full_worker_transcript_in_review: false,
        recent_history_tokens: 20_000
      },
      profile_override: null,
      effective_by_backend: []
    },
    notifications: {
      configured: true,
      transport: 'custom_command',
      manager_wake_autonomy: 'review_only',
      env_file_configured: true,
      env_file_prod_configured: true,
      env_file: canary,
      env_file_prod: `${canary}_PROD`
    }
  };

  const component = await mount(
    <ProfileConfigViewerSection
      selectedName="test-repo"
      profileConfig={{ data: config, loading: false, error: null }}
    />
  );

  await expect(component).toContainText('dev env: configured');
  await expect(component).toContainText('prod env: configured');
  await expect(component).not.toContainText(canary);
});
