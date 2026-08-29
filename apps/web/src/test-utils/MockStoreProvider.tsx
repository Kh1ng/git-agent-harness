import React from 'react';
import type { StatusSnapshot, QuotaSnapshot, ProfileIdentity } from '@git-agent-harness/contracts';
import { useGahStore } from '../store/gahStore.js';

interface LoopStatus {
  running: boolean;
  pid?: number;
  startedAt?: string;
}

export interface MockStoreProviderProps {
  children: React.ReactNode;
  statusData: StatusSnapshot | null;
}

export function MockStoreProvider({ children, statusData }: MockStoreProviderProps) {
  // Initialize the store with our test data
  React.useEffect(() => {
    // Mock fetch functions to prevent actual API calls
    useGahStore.getState().fetchStatus = async () => {};
    useGahStore.getState().fetchQuota = async () => {};
    useGahStore.getState().fetchLoopStatus = async () => {};
    
    const mockProfile: ProfileIdentity = {
      profile: 'test-profile',
      display_name: 'Test Profile',
      repo_id: 'test-repo',
      provider: 'github',
      local_path: '/test/path',
      default_target_branch: 'main',
      max_fix_attempts_per_mr: 3,
      max_implementation_failures_per_ticket: 3,
      max_open_managed_mrs: 5,
      merge_policy: 'squash',
      issue_intake_policy: {
        mode: 'autonomous',
        canonical_autonomous_label: 'autonomous',
        trusted_human_authors: [],
        trusted_bot_authors: [],
        github_issue_author_allowlist: [],
      },
    };

    const mockQuotaData: QuotaSnapshot = {
      schema_version: 1,
      generated_at: new Date().toISOString(),
      freshness: {},
      quota_checks: [],
      profile: mockProfile,
      since: '7d',
      usage: {
        entries: 0,
        attempts: 0,
        validation_pass: 0,
        success_rate: null,
        total_tokens: null,
        requests_count: null,
        actual_cost_usd: null,
        estimated_cost_usd: null,
      },
      candidates: [],
    };

    const mockLoopStatus: LoopStatus = { running: false };

    // Set the store state directly
    useGahStore.setState({
      status: {
        data: statusData,
        loading: false,
        error: null,
        fetchedAt: Date.now(),
        key: 'test'
      },
      quota: {
        data: mockQuotaData,
        loading: false,
        error: null,
        fetchedAt: Date.now(),
        key: 'test'
      },
      loopStatus: {
        data: mockLoopStatus,
        loading: false,
        error: null,
        fetchedAt: Date.now(),
        key: 'test'
      },
      loopAction: { pending: false, error: null }
    });
  }, [statusData]);

  return <>{children}</>;
}
