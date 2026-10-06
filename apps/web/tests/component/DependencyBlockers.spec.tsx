import { test, expect } from '@playwright/experimental-ct-react';
import type { DependencyBlocker, DependencyObservation, StatusSnapshot } from '@git-agent-harness/contracts';
import { OverviewPage } from '../../src/pages/OverviewPage.js';
import { WebSocketProvider } from '../../src/ws/WebSocketContext.js';
import { MockStoreProvider } from '../../src/test-utils/MockStoreProvider.js';
import React from 'react';

// Mock session for OverviewPage props
const mockSession = {
  id: 'test-session',
  providerKind: 'github' as const,
  instanceId: 'test-instance',
  status: 'idle' as const,
};

// Helper to create a mock dependency blocker
function createDependencyBlocker(
  workId: string,
  title: string,
  reasonCode: string,
  reason: string,
  dependencies: DependencyObservation[]
): DependencyBlocker {
  return {
    ticket_path: workId.replace('#', ''),
    work_id: workId,
    title,
    reason_code: reasonCode,
    reason,
    dependencies,
  };
}

// Helper to create a mock status snapshot with dependency blockers
function createMockSnapshot(dependencyBlockers: DependencyBlocker[]): StatusSnapshot {
  return {
    schema_version: 1,
    review_contract_version: 1,
    generated_at: new Date().toISOString(),
    profile: {
      profile: 'test-profile',
      display_name: 'Test Profile',
      repo_id: 'test-repo',
      provider: 'github',
      local_path: '/test/path',
      default_target_branch: 'main',
      max_fix_attempts_per_mr: 3,
      max_implementation_failures_per_ticket: 3,
      merge_policy: 'squash',
      issue_intake_policy: {
        mode: 'autonomous',
        canonical_autonomous_label: 'autonomous',
        trusted_human_authors: [],
        trusted_bot_authors: [],
        github_issue_author_allowlist: [],
      },
    },
    observations: {
      sync: { status: 'ok' },
      availability: { status: 'ok' },
      ledger: { status: 'ok' },
    },
    merge_requests: [],
    availability: [],
    recent_ledger: null,
    constraints: [],
    blockers: [],
    blocked_work_items: [],
    issue_intake_rejections: [],
    dependency_blockers: dependencyBlockers,
    errors: [],
    available_tickets: [],
    active_claims: [],
    pm_parent_states: [],
    pm_decomposition_attempt_counts: {},
    pm_max_attempts: 2,
    fix_attempt_counts: {},
    merge_attempt_counts: {},
    review_held_work_ids: [],
    publishing_allow_pr: true,
    generated_artifact_deny_patterns: [],
    max_parallel_workers: 1,
    open_managed_mr_count: 0,
    inflight_implementation_count: 0,
    implementation_intake_paused: false,
    backend_configured: {},
  };
}

// Mock store provider for testing - needs to be in a separate file for Playwright CT
// For now, we'll use a simpler approach by setting store state directly

test.describe('Dependency Blockers Component', () => {
  test.beforeEach(async ({ context }) => {
    await context.addInitScript({ content: `window.__GAH_TEST_MODE__ = true` });
  });

  test('renders dependency blockers with open state', async ({ mount }) => {
    const openDeps = createDependencyBlocker(
      '#653',
      'Test issue blocked by open dependency',
      'dependency_open',
      'Blocked by open prerequisite #652',
      [{ identity: '#652', provider: 'github', provider_state: 'OPEN', normalized_state: 'open' }]
    );
    
    const snapshot = createMockSnapshot([openDeps]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Needs attention is a table: the kind badge, the work id, and one summary cell with title, reason and blockers.
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#653/).first()).toBeVisible();
    await expect(component.getByText(/Test\ issue\ blocked\ by\ open\ dependency/).first()).toBeVisible();
    await expect(component.getByText(/Blocked\ by\ open\ prerequisite\ \#652/).first()).toBeVisible();
  });

  test('renders dependency blockers with cycle state', async ({ mount }) => {
    const cycleDeps = createDependencyBlocker(
      '#1',
      'Cyclic dependency issue',
      'dependency_cycle',
      'Dependency cycle detected: #1 -> #2 -> #1',
      [
        { identity: '#1', provider: 'github', provider_state: 'OPEN', normalized_state: 'open' },
        { identity: '#2', provider: 'github', provider_state: 'OPEN', normalized_state: 'open' }
      ]
    );
    
    const snapshot = createMockSnapshot([cycleDeps]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that the cycle dependency blocker is displayed
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#1/).first()).toBeVisible();
    await expect(component.getByText(/Cyclic\ dependency\ issue/).first()).toBeVisible();
    await expect(component.getByText(/Dependency\ cycle\ detected/).first()).toBeVisible();
    await expect(component.getByText(/\#1\ \[open\],\ \#2\ \[open\]/).first()).toBeVisible();
  });

  test('renders dependency blockers with missing state', async ({ mount }) => {
    const missingDeps = createDependencyBlocker(
      '#999',
      'Issue with missing dependency',
      'dependency_missing',
      'Could not resolve dependency #404',
      [{ identity: '#404', provider: 'github', provider_state: null, normalized_state: 'missing' }]
    );
    
    const snapshot = createMockSnapshot([missingDeps]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that the missing dependency blocker is displayed
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#999/).first()).toBeVisible();
    await expect(component.getByText(/Issue\ with\ missing\ dependency/).first()).toBeVisible();
    await expect(component.getByText(/Could\ not\ resolve\ dependency\ \#404/).first()).toBeVisible();
    await expect(component.getByText(/\#404/).first()).toBeVisible();
  });

  test('renders dependency blockers with inaccessible state', async ({ mount }) => {
    const inaccessibleDeps = createDependencyBlocker(
      '#777',
      'Issue with inaccessible dependency',
      'dependency_query_failed',
      'Permission denied accessing dependency #888',
      [{ identity: '#888', provider: 'github', provider_state: null, normalized_state: 'inaccessible' }]
    );
    
    const snapshot = createMockSnapshot([inaccessibleDeps]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that the inaccessible dependency blocker is displayed
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#777/).first()).toBeVisible();
    await expect(component.getByText(/Issue\ with\ inaccessible\ dependency/).first()).toBeVisible();
    await expect(component.getByText(/Permission\ denied/).first()).toBeVisible();
    await expect(component.getByText(/\#888/).first()).toBeVisible();
  });

  test('renders dependency blockers with unknown/error state', async ({ mount }) => {
    const unknownDeps = createDependencyBlocker(
      '#555',
      'Issue with unknown dependency state',
      'dependency_query_failed',
      'Unknown error accessing dependency #666',
      [{ identity: '#666', provider: 'github', provider_state: null, normalized_state: 'unknown' }]
    );
    
    const snapshot = createMockSnapshot([unknownDeps]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that the unknown dependency blocker is displayed
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#555/).first()).toBeVisible();
    await expect(component.getByText(/Issue\ with\ unknown\ dependency\ state/).first()).toBeVisible();
    await expect(component.getByText(/Unknown\ error\ accessing\ dependency\ \#666/).first()).toBeVisible();
    await expect(component.getByText(/\#666/).first()).toBeVisible();
  });

  test('renders dependency blockers with released state (empty blockers list)', async ({ mount }) => {
    // When dependencies are released/closed, they should NOT appear in dependency_blockers
    // This tests that the rendering correctly handles the released transition
    const snapshot = createMockSnapshot([]); // Empty - all deps released
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that no dependency blockers are displayed when all are released
    await expect(component.getByText('Dependency', { exact: true }).first()).not.toBeVisible();
  });

  test('renders multiple dependency blockers', async ({ mount }) => {
    const blocker1 = createDependencyBlocker(
      '#100',
      'First blocked issue',
      'dependency_open',
      'Waiting on #200',
      [{ identity: '#200', provider: 'github', provider_state: 'OPEN', normalized_state: 'open' }]
    );
    
    const blocker2 = createDependencyBlocker(
      '#101',
      'Second blocked issue',
      'dependency_open',
      'Waiting on #201',
      [{ identity: '#201', provider: 'github', provider_state: 'OPEN', normalized_state: 'open' }]
    );
    
    const snapshot = createMockSnapshot([blocker1, blocker2]);
    
    const component = await mount(
      <MockStoreProvider statusData={snapshot}>
        <WebSocketProvider>
          <OverviewPage 
            sessions={[mockSession]} 
            onSelectSession={() => {}} 
            onNavigate={() => {}} 
          />
        </WebSocketProvider>
      </MockStoreProvider>
    );

    // Check that both dependency blockers are displayed
    await expect(component.getByText('Dependency', { exact: true }).first()).toBeVisible();
    await expect(component.getByText(/\#100/).first()).toBeVisible();
    await expect(component.getByText(/\#101/).first()).toBeVisible();
    await expect(component.getByText(/First\ blocked\ issue/).first()).toBeVisible();
    await expect(component.getByText(/Second\ blocked\ issue/).first()).toBeVisible();
  });
});